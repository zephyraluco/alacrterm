use crate::SettingsSnapshot;
use anyhow::{Context as _, Result};
use notify::Watcher as _;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

/// Watches the parent directory so atomic file replacement does not detach it.
pub struct SettingsWatch {
    updates: async_channel::Receiver<Result<SettingsSnapshot, String>>,
    stopped: Arc<AtomicBool>,
    signal: mpsc::SyncSender<()>,
}

impl SettingsWatch {
    pub fn new(path: PathBuf) -> Result<Self> {
        let directory = path.parent().context("missing configuration directory")?;
        std::fs::create_dir_all(directory)?;
        let directory = directory.canonicalize()?;
        let path = directory.join(path.file_name().context("missing configuration filename")?);
        let watched = path.clone();
        let (signal, events) = mpsc::sync_channel(1);
        let notify_signal = signal.clone();
        let mut watcher = notify::RecommendedWatcher::new(
            move |event: notify::Result<notify::Event>| {
                if event.is_ok_and(|event| {
                    event.need_rescan()
                        || (matches!(
                            event.kind,
                            notify::EventKind::Any
                                | notify::EventKind::Create(_)
                                | notify::EventKind::Modify(_)
                                | notify::EventKind::Remove(_)
                        ) && event.paths.iter().any(|p| p == &watched))
                }) {
                    let _ = notify_signal.try_send(());
                }
            },
            notify::Config::default(),
        )?;
        watcher.watch(&directory, notify::RecursiveMode::NonRecursive)?;
        let (sender, updates) = async_channel::unbounded();
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = stopped.clone();
        std::thread::Builder::new()
            .name("settings-watch".into())
            .spawn(move || {
                let _watcher = watcher;
                let mut last = None;
                let mut seen_file = false;
                while events.recv().is_ok() {
                    while events.recv_timeout(Duration::from_millis(150)).is_ok() {
                        if worker_stopped.load(Ordering::Acquire) {
                            return;
                        }
                    }
                    if worker_stopped.load(Ordering::Acquire) {
                        return;
                    }
                    let result = match std::fs::read_to_string(&path) {
                        Ok(text) => {
                            seen_file = true;
                            serde_json::from_str(&text)
                                .map_err(anyhow::Error::from)
                                .and_then(SettingsSnapshot::parse)
                                .map_err(|e| format!("{}：{e:#}", path.display()))
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !seen_file => {
                            continue;
                        }
                        Err(e) => Err(format!("{}：{e}", path.display())),
                    };
                    let fingerprint = result
                        .as_ref()
                        .map(|s| s.document.clone())
                        .map_err(Clone::clone);
                    if last.as_ref() == Some(&fingerprint) {
                        continue;
                    }
                    last = Some(fingerprint);
                    if sender.send_blocking(result).is_err() {
                        return;
                    }
                }
            })?;
        let _ = signal.try_send(());
        Ok(Self {
            updates,
            stopped,
            signal,
        })
    }

    pub fn updates(&self) -> async_channel::Receiver<Result<SettingsSnapshot, String>> {
        self.updates.clone()
    }
}

impl Drop for SettingsWatch {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        let _ = self.signal.try_send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn receive(
        receiver: &async_channel::Receiver<Result<SettingsSnapshot, String>>,
    ) -> Result<SettingsSnapshot, String> {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(result) = receiver.try_recv() {
                return result;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "watcher notification timed out"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    #[test]
    fn atomic_replacement_invalid_edits_recovery_and_formatting_deduplication() {
        let (_cache, lease) = crate::tests::migration_fixture();
        std::fs::write(lease.path(), "{}").unwrap();
        let watch = SettingsWatch::new(lease.path().to_owned()).unwrap();
        let updates = watch.updates();
        assert_eq!(receive(&updates).unwrap().document, serde_json::json!({}));
        util::atomic_file::write(lease.path(), br#"{"theme_mode":"light"}"#).unwrap();
        assert_eq!(
            receive(&updates).unwrap().values.app.theme_mode,
            crate::AppThemeMode::Light
        );
        util::atomic_file::write(lease.path(), br#"{"terminal":{"font_size":0}}"#).unwrap();
        assert!(receive(&updates).unwrap_err().contains("font_size"));
        for size in [16, 17, 18] {
            util::atomic_file::write(
                lease.path(),
                format!("{{\"terminal\":{{\"font_size\":{size}}}}}").as_bytes(),
            )
            .unwrap();
        }
        assert_eq!(
            receive(&updates).unwrap().values.terminal.font_size,
            Some(18.)
        );
        std::fs::write(lease.path(), "{ \"terminal\": {\"font_size\":18} }\n").unwrap();
        std::thread::sleep(Duration::from_millis(350));
        assert!(updates.try_recv().is_err());
    }
}
