//! Bundled WayX process lifecycle and local display endpoints.
use anyhow::{Context, Result, bail, ensure};
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, LazyLock, Mutex,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::Semaphore,
};

/// Cookies intentionally have no Debug implementation.
#[derive(Clone)]
pub struct Endpoint {
    display: u16,
    permits: Arc<Semaphore>,
    generation: u64,
}

pub(crate) struct Wayx {
    child: Option<Child>,
    directory: PathBuf,
    pub(crate) forwarder: Endpoint,
}

/// State displayed by the application-level WayX control.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WayxStatus {
    #[default]
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed,
}

#[derive(Default)]
struct ServiceState {
    server: Option<Wayx>,
    status: WayxStatus,
    generation: u64,
    shutdown: bool,
}

#[derive(Clone, Default)]
struct WayxService {
    state: Arc<Mutex<ServiceState>>,
    cleanup: Arc<Mutex<()>>,
}
static SHARED_WAYX: LazyLock<WayxService> = LazyLock::new(WayxService::default);

/// Start WayX on a worker thread without blocking the UI or opening SSH.
pub fn start_wayx() {
    SHARED_WAYX.start();
}
/// Stop WayX asynchronously, leaving SSH and SFTP connections intact.
pub fn stop_wayx() {
    SHARED_WAYX.stop(false);
}
/// Read the current state, detecting an unexpectedly exited server.
pub fn wayx_status() -> WayxStatus {
    SHARED_WAYX.status()
}
/// Permanently stop the service during application exit.
pub fn shutdown_wayx() {
    SHARED_WAYX.stop(true);
}

impl WayxService {
    fn lock(&self) -> std::sync::MutexGuard<'_, ServiceState> {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn status(&self) -> WayxStatus {
        let mut state = self.lock();
        if state.status == WayxStatus::Running {
            let alive = state
                .server
                .as_mut()
                .and_then(|server| server.child.as_mut())
                .is_some_and(|child| matches!(child.try_wait(), Ok(None)));
            if !alive {
                state.status = WayxStatus::Failed;
                state.server.take();
            }
        }
        state.status
    }

    fn current(&self) -> Result<Endpoint> {
        ensure!(self.status() == WayxStatus::Running, "WayX is not running");
        let state = self.lock();
        ensure!(state.status == WayxStatus::Running, "WayX is not running");
        Ok(state
            .server
            .as_ref()
            .context("WayX is not running")?
            .forwarder
            .clone())
    }

    fn begin_start(&self) -> Option<u64> {
        let mut state = self.lock();
        if state.shutdown
            || matches!(
                state.status,
                WayxStatus::Starting | WayxStatus::Running | WayxStatus::Stopping
            )
        {
            return None;
        }
        state.generation += 1;
        state.status = WayxStatus::Starting;
        Some(state.generation)
    }

    fn start(&self) {
        let Some(generation) = self.begin_start() else {
            return;
        };
        let service = self.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("wayx-start".into())
            .spawn(move || {
                let result = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(anyhow::Error::from)
                    .and_then(|runtime| runtime.block_on(service.finish_start(generation)));
                if let Err(error) = result {
                    log::debug!("WayX startup: {error:#}");
                    service.fail_start(generation);
                }
            })
        {
            log::debug!("WayX worker: {error}");
            self.fail_start(generation);
        }
    }

    fn fail_start(&self, generation: u64) {
        let _cleanup = self
            .cleanup
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let server = {
            let mut state = self.lock();
            if state.generation != generation {
                return;
            }
            state.status = WayxStatus::Failed;
            state.server.take()
        };
        drop(server);
    }

    async fn finish_start(&self, generation: u64) -> Result<()> {
        {
            let mut state = self.lock();
            ensure!(
                state.generation == generation
                    && !state.shutdown
                    && state.status == WayxStatus::Starting,
                "WayX startup cancelled"
            );
            // Keep process creation and publishing ownership atomic with respect
            // to shutdown; no child can appear after the exit hook has returned.
            let mut server = Wayx::spawn()?;
            server.forwarder.generation = generation;
            state.server = Some(server);
        }
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let probe = {
                    let mut state = self.lock();
                    ensure!(
                        state.generation == generation && state.status == WayxStatus::Starting,
                        "WayX startup cancelled"
                    );
                    let server = state.server.as_mut().context("WayX stopped")?;
                    if let Some(child) = &mut server.child {
                        ensure!(child.try_wait()?.is_none(), "WayX exited during startup");
                    }
                    server.forwarder.clone()
                };
                if probe.ready().await {
                    let mut state = self.lock();
                    ensure!(
                        state.generation == generation && !state.shutdown,
                        "WayX startup cancelled"
                    );
                    state.status = WayxStatus::Running;
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .context("WayX readiness timeout")?
    }

    fn stop(&self, permanent: bool) {
        let generation = {
            let mut state = self.lock();
            if state.shutdown && !permanent {
                return;
            }
            if !permanent && state.status == WayxStatus::Stopping {
                return;
            }
            state.shutdown |= permanent;
            state.generation += 1;
            state.status = WayxStatus::Stopping;
            state.generation
        };
        let service = self.clone();
        let cleanup = move || {
            // Shutdown joins in-progress cleanup before returning. A delayed
            // worker never privately owns an untracked child process.
            let _cleanup = service
                .cleanup
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            let server = {
                let mut state = service.lock();
                if state.generation != generation {
                    return;
                }
                state.server.take()
            };
            drop(server);
            let mut state = service.lock();
            if state.generation == generation {
                state.status = WayxStatus::Stopped;
            }
        };
        if permanent {
            cleanup();
        } else {
            if std::thread::Builder::new()
                .name("wayx-stop".into())
                .spawn(cleanup)
                .is_err()
            {
                let mut state = self.lock();
                if state.generation == generation {
                    state.status = WayxStatus::Failed;
                }
            }
        }
    }
}

impl Drop for Wayx {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            #[cfg(unix)]
            if matches!(child.try_wait(), Ok(None)) {
                // Let XLibre remove its socket and display lock before forcing exit.
                // The child remains unreaped here, so its PID cannot be reused.
                unsafe {
                    libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
                }
                for _ in 0..20 {
                    if !matches!(child.try_wait(), Ok(None)) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            let _ = child.kill();
            let _ = child.wait();
            #[cfg(unix)]
            cleanup_display(self.forwarder.display, child.id());
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[cfg(unix)]
fn cleanup_display(display: u16, pid: u32) {
    use std::{
        io::Read,
        os::unix::fs::{FileTypeExt, MetadataExt},
    };
    let lock = PathBuf::from(format!("/tmp/.X{display}-lock"));
    let socket = PathBuf::from(format!("/tmp/.X11-unix/X{display}"));
    let Ok(metadata) = std::fs::symlink_metadata(&lock) else {
        return;
    };
    if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() } {
        return;
    }
    let mut text = String::new();
    let Ok(file) = std::fs::File::open(&lock) else {
        return;
    };
    if file.take(128).read_to_string(&mut text).is_err() || text.trim().parse::<u32>() != Ok(pid) {
        return;
    }
    // Never remove a different server's display. Keep our reservation until cleanup finishes.
    if let Ok(socket_metadata) = std::fs::symlink_metadata(&socket) {
        if socket_metadata.file_type().is_socket() && socket_metadata.uid() == metadata.uid() {
            let _ = std::fs::remove_file(socket);
        }
    }
    let _ = std::fs::remove_file(lock);
}

fn runtime_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("WAYX_RUNTIME_DIR") {
        return Ok(path.into());
    }
    let name = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "linux/wayx-linux-x64",
        ("macos", "aarch64") => "macos/wayx-macos-arm64",
        ("windows", "x86_64") => "windows/wayx-windows-x64",
        (os, arch) => bail!("没有打包 {os}/{arch} 的 WayX，请通过 WAYX_RUNTIME_DIR 指定运行包"),
    };
    let executable = std::env::current_exe()?;
    let cwd = std::env::current_dir()?;
    for base in executable
        .ancestors()
        .skip(1)
        .chain(cwd.ancestors())
        .chain(Path::new(env!("CARGO_MANIFEST_DIR")).ancestors())
    {
        let path = base.join("platform").join(name);
        if path.join("bin").is_dir() {
            return Ok(path);
        }
    }
    bail!("找不到 platform/{name}；请随应用分发 platform 目录或设置 WAYX_RUNTIME_DIR")
}

impl Wayx {
    fn spawn() -> Result<Self> {
        let runtime = runtime_dir()?;
        let mut reservation = None;
        for display in 100..1000u16 {
            #[cfg(unix)]
            if Path::new(&format!("/tmp/.X11-unix/X{display}")).exists()
                || Path::new(&format!("/tmp/.X{display}-lock")).exists()
            {
                continue;
            }
            #[cfg(windows)]
            if std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 6000 + display)).is_err()
            {
                continue;
            }
            let directory = std::env::temp_dir().join(format!("alacrterm-wayx-{display}"));
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&directory) {
                Ok(()) => {
                    reservation = Some((display, directory));
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        let (display, directory) = reservation.context("没有可用的 WayX 显示编号")?;
        let mut server = Self {
            child: None,
            directory,
            forwarder: Endpoint {
                display,
                permits: Arc::new(Semaphore::new(64)),
                generation: 0,
            },
        };
        let bin = runtime.join("bin");
        let mut command = Command::new(bin.join(if cfg!(windows) { "wayx.exe" } else { "wayx" }));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW; WayX owns its GUI windows.
        }
        let core = if cfg!(windows) {
            bin.join("libwayx-xserver.dll")
        } else {
            runtime.join("lib").join(if cfg!(target_os = "macos") {
                "libwayx-xserver.dylib"
            } else {
                "libwayx-xserver.so"
            })
        };
        command
            .args(["serve", "--rootless", "--core"])
            .arg(core)
            .arg("--")
            .arg(format!(":{display}"));
        #[cfg(unix)]
        command.args(["-nolisten", "tcp"]);
        #[cfg(windows)]
        command.args(["-listen", "tcp", "-nolisten", "local"]);
        command
            .arg("-ac")
            .env("WAYX_XKB_DIR", runtime.join("share/X11/xkb"))
            .env("WAYX_XKB_BIN_DIR", &bin)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(std::fs::File::create(
                server.directory.join("wayx.log"),
            )?));
        let library_var = if cfg!(windows) {
            "PATH"
        } else if cfg!(target_os = "macos") {
            "DYLD_LIBRARY_PATH"
        } else {
            "LD_LIBRARY_PATH"
        };
        let mut paths = vec![if cfg!(windows) {
            bin
        } else {
            runtime.join("lib")
        }];
        if let Some(existing) = std::env::var_os(library_var) {
            paths.extend(std::env::split_paths(&existing));
        }
        command.env(library_var, std::env::join_paths(paths)?);
        server.child = Some(
            command
                .spawn()
                .context("启动内置 WayX（请检查可执行权限及运行依赖）")?,
        );
        Ok(server)
    }
}

impl Endpoint {
    #[cfg(unix)]
    pub async fn local(&self) -> std::io::Result<tokio::net::UnixStream> {
        tokio::net::UnixStream::connect(format!("/tmp/.X11-unix/X{}", self.display)).await
    }
    #[cfg(windows)]
    pub async fn local(&self) -> std::io::Result<tokio::net::TcpStream> {
        tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, 6000 + self.display)).await
    }

    async fn ready(&self) -> bool {
        let probe = async {
            let mut local = self.local().await?;
            let setup = [b'l', 0, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0];
            local.write_all(&setup).await?;
            let mut response = [0; 8];
            local.read_exact(&mut response).await?;
            Ok::<bool, std::io::Error>(response[0] == 1)
        };
        matches!(
            tokio::time::timeout(Duration::from_secs(1), probe).await,
            Ok(Ok(true))
        )
    }

    /// Obtain a snapshot of the currently running display.
    pub fn current() -> Result<Self> { SHARED_WAYX.current() }
    /// Limit concurrent connections to this display generation.
    pub fn acquire(&self) -> Result<tokio::sync::OwnedSemaphorePermit> {
        self.permits.clone().try_acquire_owned().context("WayX connection limit reached")
    }
    /// Whether this endpoint still belongs to the running server.
    pub fn is_current(&self) -> bool {
        let state = SHARED_WAYX.lock();
        state.generation == self.generation && state.status == WayxStatus::Running
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shutdown_invalidates_pending_start_and_prevents_restart() {
        let service = WayxService::default();
        let old = service.begin_start().unwrap();
        assert!(service.begin_start().is_none());
        service.stop(true);
        service.fail_start(old);
        assert_eq!(service.status(), WayxStatus::Stopped);
        assert!(service.begin_start().is_none());
    }

    #[tokio::test]
    #[ignore = "requires a host display and bundled WayX runtime"]
    async fn manual_lifecycle_restarts_without_ssh_and_cancels_stale_startup() {
        let service = WayxService::default();
        let generation = service.begin_start().unwrap();
        service.finish_start(generation).await.unwrap();
        assert_eq!(service.status(), WayxStatus::Running);
        let first = service.current().unwrap();
        let directory = service.lock().server.as_ref().unwrap().directory.clone();
        assert!(!directory.join("Xauthority").exists());
        assert!(first.ready().await);
        service.stop(false);
        tokio::time::timeout(Duration::from_secs(5), async {
            while service.status() == WayxStatus::Stopping {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(service.status(), WayxStatus::Stopped);
        assert!(!directory.exists());
        assert!(service.current().is_err());
        let next = service.begin_start().unwrap();
        service.fail_start(generation); // An old worker must not clobber the new state.
        assert_eq!(service.status(), WayxStatus::Starting);
        service.finish_start(next).await.unwrap();
        let second = service.current().unwrap();
        assert!(second.ready().await);
        assert_ne!(first.generation, second.generation);
        let directory = service.lock().server.as_ref().unwrap().directory.clone();
        service.stop(false);
        service.stop(true); // Exit while asynchronous stop is still in progress.
        assert!(!directory.exists());
        assert!(service.begin_start().is_none());
    }

    #[tokio::test]
    #[ignore = "requires a host display and bundled WayX runtime"]
    async fn shutdown_during_startup_does_not_restore_running_state() {
        let service = WayxService::default();
        let generation = service.begin_start().unwrap();
        let mut startup = Box::pin(service.finish_start(generation));
        assert!(futures::poll!(&mut startup).is_pending());
        let directory = service.lock().server.as_ref().unwrap().directory.clone();
        service.stop(true);
        assert!(startup.await.is_err());
        service.fail_start(generation);
        assert_eq!(service.status(), WayxStatus::Stopped);
        assert!(!directory.exists());
    }
}
