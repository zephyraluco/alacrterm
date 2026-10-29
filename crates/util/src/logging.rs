//! Process-local tracing sink. No business events are emitted here.
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::mpsc,
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use time::{Date, OffsetDateTime, macros::format_description};
use tracing_appender::{
    non_blocking::WorkerGuard,
    rolling::{RollingFileAppender, Rotation},
};
use tracing_subscriber::{EnvFilter, fmt::writer::BoxMakeWriter};

const DEFAULT_FILTER: &str = "warn,alacrterm=info,alacrterm_update_helper=info,terminal=info,terminal_view=info,ssh=info,settings=info,session=info,platform=info,transfer=info,updater=info,util=info";
const CLEANUP_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Keep this value alive until all application shutdown work has finished.
pub struct LoggingGuard {
    worker: Option<WorkerGuard>,
    maintenance: Option<(mpsc::Sender<()>, JoinHandle<()>)>,
}
impl Drop for LoggingGuard {
    fn drop(&mut self) {
        if let Some((stop, task)) = self.maintenance.take() {
            let _ = stop.send(());
            let _ = task.join();
        }
        // WorkerGuard drains the bounded queue before closing the file.
        drop(self.worker.take());
    }
}

pub fn log_directory() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        dirs::data_local_dir().map(|path| path.join("Alacrterm/logs"))
    }
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir().map(|path| path.join("Library/Logs/Alacrterm"))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        dirs::state_dir().map(|path| path.join("alacrterm/logs"))
    }
}

fn filter(value: Option<&str>) -> EnvFilter {
    value
        .filter(|value| !value.trim().is_empty())
        .and_then(|value| EnvFilter::builder().with_regex(false).parse(value).ok())
        .unwrap_or_else(|| {
            EnvFilter::builder()
                .with_regex(false)
                .parse(DEFAULT_FILTER)
                .expect("valid built-in log filter")
        })
}

fn appender(directory: &Path, application: &str) -> io::Result<RollingFileAppender> {
    // Directory privacy is set only when we create it; existing directory permissions are preserved.
    let existed = directory.exists();
    fs::create_dir_all(directory)?;
    #[cfg(unix)]
    if !existed {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    let _ = existed;
    let instance = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let prefix = format!("{application}-{profile}-{}-{instance}", std::process::id());
    RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(prefix)
        .filename_suffix("log")
        .build(directory)
        .map_err(io::Error::other)
}

/// Initializes files when available, otherwise keeps existing diagnostics on stderr.
/// No panic hook, additional events or application spans are installed.
pub fn init(application: &'static str) -> LoggingGuard {
    init_in(application, log_directory())
}

fn init_in(application: &'static str, directory: Option<PathBuf>) -> LoggingGuard {
    let file = directory
        .as_deref()
        .and_then(|directory| appender(directory, application).ok());
    let (writer, worker) = if let Some(file) = file {
        let (writer, guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
            .lossy(true)
            .finish(file);
        (BoxMakeWriter::new(writer), Some(guard))
    } else {
        (BoxMakeWriter::new(io::stderr), None)
    };
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_target(true)
        .with_env_filter(filter(std::env::var("RUST_LOG").ok().as_deref()))
        .with_writer(writer)
        .finish();
    let installed = tracing::subscriber::set_global_default(subscriber).is_ok();
    if installed {
        // Dependencies still using log are handled by the same subscriber and filter.
        let _ = tracing_log::LogTracer::init();
    }
    let maintenance = if installed && worker.is_some() {
        directory.and_then(|directory| {
            let (stop, receiver) = mpsc::channel();
            thread::Builder::new()
                .name("log-retention".into())
                .spawn(move || {
                    loop {
                        cleanup_at(&directory, OffsetDateTime::now_utc().date());
                        match receiver.recv_timeout(CLEANUP_INTERVAL) {
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                            _ => break,
                        }
                    }
                })
                .ok()
                .map(|task| (stop, task))
        })
    } else {
        None
    };
    LoggingGuard {
        worker,
        maintenance,
    }
}

fn log_date(name: &str) -> Option<Date> {
    let rest = name
        .strip_prefix("alacrterm-update-helper-")
        .or_else(|| name.strip_prefix("alacrterm-"))?;
    let (instance, date) = rest.strip_suffix(".log")?.rsplit_once('.')?;
    let instance = instance
        .strip_prefix("debug-")
        .or_else(|| instance.strip_prefix("release-"))
        .unwrap_or(instance);
    let (pid, nonce) = instance.split_once('-')?;
    if pid.is_empty()
        || nonce.is_empty()
        || !pid
            .bytes()
            .chain(nonce.bytes())
            .all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    Date::parse(date, format_description!("[year]-[month]-[day]")).ok()
}

/// Retain today and the preceding six UTC calendar dates, across all instances.
/// Only our own regularly named log files are removed; no size limits apply.
fn cleanup_at(directory: &Path, today: Date) {
    let cutoff = today - time::Duration::days(6);
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_file())
            && log_date(&entry.file_name().to_string_lossy()).is_some_and(|date| date < cutoff)
        {
            // An open Windows log or a temporarily inaccessible file can be retried next hour.
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    #[test]
    fn seven_calendar_days_are_retained_without_deleting_other_files() {
        let temp = tempfile::tempdir().unwrap();
        for name in [
            "alacrterm-12-10.2026-10-22.log",
            "alacrterm-update-helper-12-11.2026-10-22.log",
            "alacrterm-12-10.2026-10-23.log",
            "alacrterm-99-42.2026-10-29.log",
            "notes.log",
            "alacrterm-user-notes.2026-10-01.log",
            "installer.log",
        ] {
            fs::write(temp.path().join(name), b"fixture").unwrap();
        }
        let directory = temp.path().join("alacrterm-22-2.2026-10-01.log");
        fs::create_dir(&directory).unwrap();
        cleanup_at(temp.path(), date!(2026 - 10 - 29));
        assert!(!temp.path().join("alacrterm-12-10.2026-10-22.log").exists());
        assert!(
            !temp
                .path()
                .join("alacrterm-update-helper-12-11.2026-10-22.log")
                .exists()
        );
        for name in [
            "alacrterm-12-10.2026-10-23.log",
            "alacrterm-99-42.2026-10-29.log",
            "notes.log",
            "alacrterm-user-notes.2026-10-01.log",
            "installer.log",
        ] {
            assert!(temp.path().join(name).exists(), "removed {name}");
        }
        assert!(directory.is_dir());
    }

    #[test]
    fn writers_are_isolated_and_flush_before_shutdown() {
        let temp = tempfile::tempdir().unwrap();
        for message in ["first instance", "second instance"] {
            let file = appender(temp.path(), "alacrterm").unwrap();
            let (writer, guard) = tracing_appender::non_blocking(file);
            let subscriber = tracing_subscriber::fmt()
                .with_ansi(false)
                .with_writer(writer)
                .finish();
            tracing::subscriber::with_default(subscriber, || tracing::info!("{message}"));
            drop(guard);
        }
        let paths: Vec<_> = fs::read_dir(temp.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .collect();
        assert_eq!(paths.len(), 2);
        let text: String = paths
            .iter()
            .map(|path| fs::read_to_string(path).unwrap())
            .collect();
        assert!(text.contains("first instance") && text.contains("second instance"));
        assert!(!text.contains('\u{1b}'));
        assert!(
            paths
                .iter()
                .all(|path| log_date(path.file_name().unwrap().to_str().unwrap()).is_some())
        );
    }

    #[test]
    fn process_initialization_bridges_legacy_logs_filters_and_flushes() {
        const CHILD_DIRECTORY: &str = "ALACRTERM_TEST_LOG_DIRECTORY";
        if let Some(directory) = std::env::var_os(CHILD_DIRECTORY) {
            let guard = init_in("alacrterm", Some(directory.into()));
            tracing::info!(target: "alacrterm", "application message");
            log::warn!(target: "legacy_dependency", "legacy message");
            tracing::debug!(target: "alacrterm", "filtered application debug");
            tracing::info!(target: "dependency", "filtered dependency info");
            drop(guard);
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let stale = temp.path().join("alacrterm-12-1.2020-01-01.log");
        fs::write(&stale, b"old").unwrap();
        for _ in 0..2 {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "logging::tests::process_initialization_bridges_legacy_logs_filters_and_flushes"])
                .env(CHILD_DIRECTORY, temp.path()).env_remove("RUST_LOG").output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert!(!stale.exists());
        let files: Vec<_> = fs::read_dir(temp.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .collect();
        assert_eq!(files.len(), 2);
        for file in files {
            let text = fs::read_to_string(file).unwrap();
            assert!(text.contains("application message") && text.contains("legacy message"));
            assert!(!text.contains("filtered"));
        }
    }

    #[test]
    fn unavailable_log_directory_falls_back_to_stderr_without_overwriting_it() {
        const CHILD_FILE: &str = "ALACRTERM_TEST_BLOCKED_LOG_DIRECTORY";
        if let Some(path) = std::env::var_os(CHILD_FILE) {
            let guard = init_in("alacrterm", Some(path.into()));
            tracing::error!(target: "alacrterm", "fallback diagnostic");
            drop(guard);
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("existing-file");
        fs::write(&file, b"preserve").unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "logging::tests::unavailable_log_directory_falls_back_to_stderr_without_overwriting_it"])
            .env(CHILD_FILE, &file).env_remove("RUST_LOG").output().unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("fallback diagnostic"));
        assert_eq!(fs::read(&file).unwrap(), b"preserve");
    }

    #[test]
    fn bad_filters_fall_back_and_existing_files_do_not_become_directories() {
        assert!(
            filter(Some("bad[[[="))
                .to_string()
                .contains("alacrterm=info")
        );
        assert!(
            filter(Some("alacrterm=debug"))
                .to_string()
                .contains("alacrterm=debug")
        );
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("blocked");
        fs::write(&file, b"existing").unwrap();
        assert!(appender(&file, "alacrterm").is_err());
        assert_eq!(fs::read(file).unwrap(), b"existing");
    }
}
