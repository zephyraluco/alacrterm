//! Generic file-backed resources. Defaults survive invalid edits and file removal.
use anyhow::{Context, Result, bail};
use notify::Watcher as _;
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

#[derive(Debug, PartialEq)]
pub struct Resource<T> {
    pub id: String,
    pub value: Arc<T>,
}
impl<T> Clone for Resource<T> {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            value: self.value.clone(),
        }
    }
}
impl<T> Resource<T> {
    pub fn new(id: impl Into<String>, value: T) -> Self {
        Self {
            id: id.into(),
            value: Arc::new(value),
        }
    }
}

/// A successful directory snapshot, including the last valid version of bad files.
#[derive(Debug, PartialEq)]
pub struct DirectoryResources<T>(BTreeMap<PathBuf, Resource<T>>);
impl<T> Clone for DirectoryResources<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<T> Default for DirectoryResources<T> {
    fn default() -> Self {
        Self(BTreeMap::new())
    }
}
impl<T: PartialEq> DirectoryResources<T> {
    pub fn reload(
        &mut self,
        directory: &Path,
        loader: &impl Fn(&Path, &str) -> Result<Resource<T>>,
    ) -> Result<()> {
        let mut paths = std::fs::read_dir(directory)?
            .map(|entry| entry.map(|e| e.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        paths.retain(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "json"));
        paths.sort();
        let mut next = self.0.clone();
        next.retain(|path, _| paths.contains(path));
        for path in paths {
            let result = std::fs::read_to_string(&path)
                .map_err(anyhow::Error::from)
                .and_then(|text| loader(&path, &text));
            match result {
                Ok(resource) => {
                    next.insert(path, resource);
                }
                Err(error) => log::warn!("Resource reload {}: {error:#}", path.display()),
            }
        }
        let mut ids = std::collections::HashSet::new();
        for resource in next.values() {
            if resource.id.is_empty() || !ids.insert(&resource.id) {
                bail!("empty or duplicate resource ID: {}", resource.id);
            }
        }
        self.0 = next;
        Ok(())
    }
}

/// In-memory registry; UI frameworks can observe changes through their own adapter.
pub struct ResourceRegistry<T> {
    defaults: HashMap<String, Arc<T>>,
    external: HashMap<String, Arc<T>>,
    resources: HashMap<String, Arc<T>>,
}
impl<T> Default for ResourceRegistry<T> {
    fn default() -> Self {
        Self {
            defaults: HashMap::new(),
            external: HashMap::new(),
            resources: HashMap::new(),
        }
    }
}
impl<T: PartialEq> ResourceRegistry<T> {
    pub fn register_defaults(&mut self, resources: impl IntoIterator<Item = Resource<T>>) {
        for resource in resources {
            if !self.external.contains_key(&resource.id) {
                self.resources
                    .insert(resource.id.clone(), resource.value.clone());
            }
            self.defaults.insert(resource.id, resource.value);
        }
    }
    pub fn get(&self, id: &str) -> Option<Arc<T>> {
        self.resources.get(id).cloned()
    }
    pub fn entries(&self) -> &HashMap<String, Arc<T>> {
        &self.resources
    }
    /// Returns false when the effective values are unchanged, including formatting-only edits.
    pub fn apply(&mut self, directory: DirectoryResources<T>) -> bool {
        let mut next = self.defaults.clone();
        self.external = directory
            .0
            .into_values()
            .map(|entry| (entry.id, entry.value))
            .collect();
        next.extend(
            self.external
                .iter()
                .map(|(id, value)| (id.clone(), value.clone())),
        );
        if next == self.resources {
            return false;
        }
        self.resources = next;
        true
    }
}

/// Owns a single watcher and background loader. Dropping it closes pending delivery.
pub struct DirectoryWatch<T> {
    updates: async_channel::Receiver<DirectoryResources<T>>,
    stopped: Arc<AtomicBool>,
    signal: mpsc::SyncSender<()>,
}
impl<T: PartialEq + Send + Sync + 'static> DirectoryWatch<T> {
    pub fn new(
        directory: PathBuf,
        loader: impl Fn(&Path, &str) -> Result<Resource<T>> + Send + 'static,
    ) -> Result<Self> {
        std::fs::create_dir_all(&directory)?;
        let (signal, events) = mpsc::sync_channel(1);
        let notify_signal = signal.clone();
        let mut watcher = notify::RecommendedWatcher::new(
            move |event: notify::Result<notify::Event>| match event {
                Ok(event)
                    if event.need_rescan()
                        || matches!(
                            event.kind,
                            notify::EventKind::Any
                                | notify::EventKind::Create(_)
                                | notify::EventKind::Modify(_)
                                | notify::EventKind::Remove(_)
                        ) =>
                {
                    let _ = notify_signal.try_send(());
                }
                Err(error) => log::warn!("Resource watcher: {error}"),
                _ => {}
            },
            notify::Config::default().with_follow_symlinks(false),
        )?;
        // Establish the watch before the initial scan, so saves during loading are queued.
        watcher.watch(&directory, notify::RecursiveMode::NonRecursive)?;
        let (sender, updates) = async_channel::bounded(1);
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = stopped.clone();
        std::thread::Builder::new()
            .name("resource-loader".into())
            .spawn(move || {
                let _watcher = watcher;
                let mut resources = DirectoryResources::default();
                let mut first = true;
                loop {
                    if worker_stopped.load(Ordering::Acquire) {
                        break;
                    }
                    let previous = resources.clone();
                    match resources.reload(&directory, &loader) {
                        Ok(()) if first || resources != previous => {
                            if sender.send_blocking(resources.clone()).is_err() {
                                break;
                            }
                        }
                        Err(error) => {
                            log::warn!("Resource directory {}: {error:#}", directory.display())
                        }
                        _ => {}
                    }
                    first = false;
                    if events.recv().is_err() {
                        break;
                    }
                    while events.recv_timeout(Duration::from_millis(150)).is_ok() {
                        if worker_stopped.load(Ordering::Acquire) {
                            return;
                        }
                    }
                }
            })
            .context("start resource loader")?;
        Ok(Self {
            updates,
            stopped,
            signal,
        })
    }
    pub fn updates(&self) -> async_channel::Receiver<DirectoryResources<T>> {
        self.updates.clone()
    }
}
impl<T> Drop for DirectoryWatch<T> {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.updates.close();
        let _ = self.signal.try_send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn load(path: &Path, text: &str) -> Result<Resource<u32>> {
        Ok(Resource::new(
            path.file_stem().unwrap().to_string_lossy(),
            text.parse::<u32>()?,
        ))
    }
    #[test]
    fn invalid_edits_retain_old_values_and_deletion_restores_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.json");
        let mut registry = ResourceRegistry::default();
        registry.register_defaults([Resource::new("sample", 1)]);
        let mut files = DirectoryResources::default();
        std::fs::write(&path, "2").unwrap();
        files.reload(dir.path(), &load).unwrap();
        assert!(registry.apply(files.clone()));
        assert_eq!(*registry.get("sample").unwrap(), 2);
        std::fs::write(&path, "incomplete").unwrap();
        files.reload(dir.path(), &load).unwrap();
        assert!(!registry.apply(files.clone()));
        std::fs::remove_file(&path).unwrap();
        files.reload(dir.path(), &load).unwrap();
        assert!(registry.apply(files));
        assert_eq!(*registry.get("sample").unwrap(), 1);
    }
    #[test]
    fn additions_removals_and_duplicate_ids_are_transactional() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.json"), "3").unwrap();
        let mut files = DirectoryResources::default();
        files.reload(dir.path(), &load).unwrap();
        let old = files.clone();
        std::fs::write(dir.path().join("b.json"), "4").unwrap();
        assert!(
            files
                .reload(dir.path(), &|_, _| Ok(Resource::new("duplicate", 0)))
                .is_err()
        );
        assert_eq!(files, old);
        files.reload(dir.path(), &load).unwrap();
        let mut registry = ResourceRegistry::default();
        assert!(registry.apply(files.clone()));
        assert_eq!(registry.entries().len(), 2);
        std::fs::remove_file(dir.path().join("b.json")).unwrap();
        files.reload(dir.path(), &load).unwrap();
        registry.apply(files);
        assert!(registry.get("b").is_none());
    }
    #[test]
    fn equal_values_do_not_refresh_and_external_values_survive_default_updates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.json");
        let mut registry = ResourceRegistry::default();
        registry.register_defaults([Resource::new("sample", 1)]);
        let mut files = DirectoryResources::default();
        std::fs::write(&path, "1").unwrap();
        files.reload(dir.path(), &load).unwrap();
        assert!(!registry.apply(files.clone()));
        registry.register_defaults([Resource::new("sample", 9)]);
        assert_eq!(*registry.get("sample").unwrap(), 1);
        let previous = files.clone();
        std::fs::write(&path, "01").unwrap();
        files.reload(dir.path(), &load).unwrap();
        assert_eq!(files, previous);
        std::fs::remove_file(path).unwrap();
        files.reload(dir.path(), &load).unwrap();
        assert!(registry.apply(files));
        assert_eq!(*registry.get("sample").unwrap(), 9);
    }
    fn receive<T>(receiver: &async_channel::Receiver<T>) -> T {
        let start = std::time::Instant::now();
        loop {
            if let Ok(value) = receiver.try_recv() {
                return value;
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "watch update timed out"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    fn watcher_loads_initial_state_atomic_replacement_and_stops_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.json");
        std::fs::write(&path, "1").unwrap();
        let watch = DirectoryWatch::new(dir.path().to_owned(), load).unwrap();
        let receiver = watch.updates();
        let mut registry = ResourceRegistry::default();
        registry.apply(receive(&receiver));
        assert_eq!(*registry.get("sample").unwrap(), 1);
        std::fs::write(dir.path().join("replacement.tmp"), "2").unwrap();
        std::fs::rename(dir.path().join("replacement.tmp"), &path).unwrap();
        registry.apply(receive(&receiver));
        assert_eq!(*registry.get("sample").unwrap(), 2);
        drop(watch);
        assert!(receiver.is_closed());
    }
}
