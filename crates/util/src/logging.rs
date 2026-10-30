//! Process-local tracing sink. No business events are emitted here.
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::mpsc,
    thread::{self, JoinHandle},
    time::Duration,
};
use time::{Date, OffsetDateTime, macros::format_description};
use tracing_appender::non_blocking::WorkerGuard;
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

/// Raw WayX output is separate from formatted application logs.
pub fn wayx_log_directory() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        log_directory().map(|directory| directory.join("wayx"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        log_directory().and_then(|directory| directory.parent().map(|parent| parent.join("wayx")))
    }
}

/// A raw byte writer with the same UTC daily rotation and retention as app logs.
pub struct OutputLog {
    file: DailyFile,
    _guard: LoggingGuard,
}

impl io::Write for OutputLog {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        io::Write::write(&mut self.file, bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        io::Write::flush(&mut self.file)
    }
}

pub fn output_log(directory: &Path, stream: &str) -> io::Result<OutputLog> {
    if !matches!(stream, "wayx" | "wayx-err") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unknown output stream",
        ));
    }
    let file = appender(directory, stream)?;
    Ok(OutputLog {
        file,
        _guard: LoggingGuard {
            worker: None,
            maintenance: retention(directory.to_owned()),
        },
    })
}

fn retention(directory: PathBuf) -> Option<(mpsc::Sender<()>, JoinHandle<()>)> {
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

/// tracing-appender's filename separator is fixed to a dot; use an append-only
/// writer here for the application's `<program>-<UTC date>.log` convention.
struct DailyFile {
    directory: PathBuf,
    application: String,
    date: Date,
    file: fs::File,
}

impl DailyFile {
    fn open(directory: &Path, application: &str, date: Date) -> io::Result<fs::File> {
        let mut options = fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        options.open(directory.join(format!("{application}-{date}.log")))
    }

    fn rotate_at(&mut self, date: Date) -> io::Result<()> {
        if self.date != date {
            let file = Self::open(&self.directory, &self.application, date)?;
            self.file = file;
            self.date = date;
        }
        Ok(())
    }
}

impl io::Write for DailyFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.rotate_at(OffsetDateTime::now_utc().date())?;
        io::Write::write(&mut self.file, bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        io::Write::flush(&mut self.file)
    }
}

fn appender(directory: &Path, application: &str) -> io::Result<DailyFile> {
    let existed = directory.exists();
    fs::create_dir_all(directory)?;
    #[cfg(unix)]
    if !existed {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    let _ = existed;
    let date = OffsetDateTime::now_utc().date();
    Ok(DailyFile {
        directory: directory.to_owned(),
        application: application.to_owned(),
        date,
        file: DailyFile::open(directory, application, date)?,
    })
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
        directory.and_then(retention)
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
        .or_else(|| name.strip_prefix("wayx-err-"))
        .or_else(|| name.strip_prefix("wayx-"))
        .or_else(|| name.strip_prefix("alacrterm-"))?;
    let stem = rest.strip_suffix(".log")?;
    if let Ok(date) = Date::parse(stem, format_description!("[year]-[month]-[day]")) {
        return Some(date);
    }
    // Continue retiring recognized files created by previous naming schemes.
    let (instance, date) = stem.rsplit_once('.')?;
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
    fn exact_program_date_names_rotate_without_overwriting_previous_days() {
        use io::Write as _;
        let directory = tempfile::tempdir().unwrap();
        let today = OffsetDateTime::now_utc().date();
        let yesterday = today - time::Duration::days(1);
        for application in ["alacrterm", "alacrterm-update-helper", "wayx", "wayx-err"] {
            let mut writer = appender(directory.path(), application).unwrap();
            writer.rotate_at(yesterday).unwrap();
            writer.file.write_all(b"previous day\n").unwrap();
            writer.write_all(b"today\n").unwrap();
            drop(writer);
            appender(directory.path(), application)
                .unwrap()
                .write_all(b"restart\n")
                .unwrap();
            assert_eq!(
                fs::read(
                    directory
                        .path()
                        .join(format!("{application}-{yesterday}.log"))
                )
                .unwrap(),
                b"previous day\n"
            );
            assert_eq!(
                fs::read(directory.path().join(format!("{application}-{today}.log"))).unwrap(),
                b"today\nrestart\n"
            );
        }
    }

    #[test]
    fn raw_output_uses_utc_dated_files_preserves_bytes_and_cleans_old_logs() {
        use io::Write as _;
        let directory = tempfile::tempdir().unwrap();
        let stale = directory.path().join("wayx-release-12-10.2020-01-01.log");
        fs::write(&stale, b"old").unwrap();
        let unrelated = directory.path().join("notes.log");
        fs::write(&unrelated, b"keep").unwrap();
        let today = OffsetDateTime::now_utc().date();
        for (stream, bytes) in [
            ("wayx", b"stdout\n".as_slice()),
            ("wayx-err", b"stderr\xff\n".as_slice()),
        ] {
            let mut log = output_log(directory.path(), stream).unwrap();
            log.write_all(bytes).unwrap();
            log.flush().unwrap();
            drop(log);
            let paths: Vec<_> = fs::read_dir(directory.path())
                .unwrap()
                .flatten()
                .filter(|entry| {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    name.starts_with(&format!("{stream}-")) && log_date(&name) == Some(today)
                })
                .collect();
            let matching = paths
                .iter()
                .any(|entry| fs::read(entry.path()).unwrap() == bytes);
            assert!(matching);
        }
        assert!(!stale.exists());
        assert!(unrelated.exists());
    }

    #[test]
    fn seven_calendar_days_are_retained_without_deleting_other_files() {
        let temp = tempfile::tempdir().unwrap();
        for name in [
            "alacrterm-2026-10-22.log",
            "wayx-2026-10-22.log",
            "wayx-err-2026-10-22.log",
            "alacrterm-update-helper-2026-10-22.log",
            "alacrterm-12-10.2026-10-22.log",
            "wayx-debug-12-10.2026-10-22.log",
            "wayx-err-release-12-10.2026-10-22.log",
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
        for name in ["alacrterm", "wayx", "wayx-err", "alacrterm-update-helper"] {
            assert!(!temp.path().join(format!("{name}-2026-10-22.log")).exists());
        }
        assert!(!temp.path().join("alacrterm-12-10.2026-10-22.log").exists());
        assert!(
            !temp
                .path()
                .join("alacrterm-update-helper-12-11.2026-10-22.log")
                .exists()
        );
        assert!(!temp.path().join("wayx-debug-12-10.2026-10-22.log").exists());
        assert!(
            !temp
                .path()
                .join("wayx-err-release-12-10.2026-10-22.log")
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
    fn writers_append_to_the_same_daily_file_and_flush_before_shutdown() {
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
        assert_eq!(paths.len(), 1);
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
        assert_eq!(files.len(), 1);
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
