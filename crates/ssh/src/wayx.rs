//! Bundled WayX lifecycle and authenticated SSH X11 channel bridging.
use anyhow::{Context, Result, bail, ensure};
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    sync::Semaphore,
};

/// Cookies intentionally have no Debug implementation.
#[derive(Clone)]
pub(crate) struct Forwarder {
    display: u16,
    fake: [u8; 16],
    real: [u8; 16],
    permits: Arc<Semaphore>,
}

pub(crate) struct Wayx {
    child: Option<Child>,
    directory: PathBuf,
    pub(crate) forwarder: Forwarder,
}

/// The application owns one server; SSH threads only borrow per-session forwarders.
#[derive(Default)]
struct ServiceState {
    server: Option<Wayx>,
    ready: bool,
    shutdown: bool,
}

#[derive(Default)]
struct WayxService {
    state: Mutex<ServiceState>,
    // Executor-independent lock: SSH sessions run on separate Tokio runtimes.
    startup: futures::lock::Mutex<()>,
}

static SHARED_WAYX: LazyLock<WayxService> = LazyLock::new(WayxService::default);

pub(crate) async fn shared_forwarder() -> Result<Forwarder> {
    SHARED_WAYX.acquire().await
}

/// Stop the shared WayX server when the application quits. Idempotent; subsequent
/// X11 connection attempts are rejected so background threads cannot restart it.
pub fn shutdown_wayx() {
    SHARED_WAYX.shutdown();
}

impl WayxService {
    fn lock(&self) -> std::sync::MutexGuard<'_, ServiceState> {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    async fn acquire(&self) -> Result<Forwarder> {
        let _startup = self.startup.lock().await;
        {
            let mut state = self.lock();
            ensure!(!state.shutdown, "应用正在退出，WayX 已关闭");
            if let Some(server) = &mut state.server {
                if server.child.as_mut().unwrap().try_wait()?.is_none() {
                    if state.ready {
                        return Ok(state.server.as_ref().unwrap().forwarder.for_session());
                    }
                } else {
                    // A crashed server is replaced on the next connection attempt.
                    state.server.take();
                    state.ready = false;
                }
            }
            if state.server.is_none() {
                // Publish ownership before awaiting readiness, so shutdown can
                // stop a process even while another thread is starting it.
                state.server = Some(Wayx::spawn()?);
                state.ready = false;
            }
        }
        let result = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let probe = {
                    let mut state = self.lock();
                    ensure!(!state.shutdown, "应用正在退出，WayX 已关闭");
                    let server = state.server.as_mut().context("WayX 已关闭")?;
                    if let Some(status) = server.child.as_mut().unwrap().try_wait()? {
                        use std::io::Read;
                        let mut log = String::new();
                        if let Ok(file) = std::fs::File::open(server.directory.join("wayx.log")) {
                            let _ = file.take(4096).read_to_string(&mut log);
                        }
                        bail!("WayX 提前退出 ({status}): {log}");
                    }
                    server.forwarder.clone()
                };
                if probe.ready().await {
                    let mut state = self.lock();
                    ensure!(!state.shutdown, "应用正在退出，WayX 已关闭");
                    state.ready = true;
                    return Ok(probe.for_session());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .context("等待 WayX 就绪超时")
        .and_then(|result| result);
        if result.is_err() {
            let server = {
                let mut state = self.lock();
                state.ready = false;
                state.server.take()
            };
            drop(server);
        }
        result
    }

    fn shutdown(&self) {
        let server = {
            let mut state = self.lock();
            state.shutdown = true;
            state.ready = false;
            state.server.take()
        };
        drop(server);
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

fn authority(display: u16, cookie: &[u8; 16]) -> Vec<u8> {
    let mut bytes = 65535u16.to_be_bytes().to_vec(); // FamilyWild
    for field in [
        b"".as_slice(),
        display.to_string().as_bytes(),
        b"MIT-MAGIC-COOKIE-1",
        cookie,
    ] {
        bytes.extend_from_slice(&(field.len() as u16).to_be_bytes());
        bytes.extend_from_slice(field);
    }
    bytes
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
            forwarder: Forwarder {
                display,
                fake: rand::random(),
                real: rand::random(),
                permits: Arc::new(Semaphore::new(64)),
            },
        };
        let auth = server.directory.join("Xauthority");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        std::io::Write::write_all(
            &mut options.open(&auth)?,
            &authority(display, &server.forwarder.real),
        )?;
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
            .arg("-auth")
            .arg(&auth)
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

impl Forwarder {
    fn for_session(&self) -> Self {
        Self {
            fake: rand::random(),
            ..self.clone()
        }
    }

    pub(crate) fn cookie_hex(&self) -> String {
        self.fake.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[cfg(unix)]
    async fn local(&self) -> std::io::Result<tokio::net::UnixStream> {
        tokio::net::UnixStream::connect(format!("/tmp/.X11-unix/X{}", self.display)).await
    }
    #[cfg(windows)]
    async fn local(&self) -> std::io::Result<tokio::net::TcpStream> {
        tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, 6000 + self.display)).await
    }

    async fn ready(&self) -> bool {
        let probe = async {
            let mut local = self.local().await?;
            let mut setup = vec![b'l', 0, 11, 0, 0, 0, 18, 0, 16, 0, 0, 0];
            setup.extend_from_slice(b"MIT-MAGIC-COOKIE-1\0\0");
            setup.extend_from_slice(&self.real);
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

    pub(crate) async fn accept(
        &self,
        channel: russh::Channel<russh::client::Msg>,
        reply: russh::client::ChannelOpenHandle,
    ) {
        let Ok(permit) = self.permits.clone().try_acquire_owned() else {
            return;
        };
        reply.accept().await;
        let forwarder = self.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let result: Result<()> = async {
                let mut remote = channel.into_stream();
                let prefix = tokio::time::timeout(
                    Duration::from_secs(10),
                    validated_setup(&mut remote, &forwarder.fake, &forwarder.real),
                )
                .await??;
                // Authentication must complete before opening any local X connection.
                let mut local = forwarder.local().await?;
                local.write_all(&prefix).await?;
                tokio::io::copy_bidirectional(&mut remote, &mut local).await?;
                Ok(())
            }
            .await;
            if let Err(error) = result {
                log::warn!("X11 通道关闭:{error}");
            }
        });
    }
}

// Adapted from zephyraluco/WayX src/ssh.rs (MIT, see docs/wayx-LICENSE).
async fn validated_setup<R: AsyncRead + Unpin>(
    stream: &mut R,
    fake: &[u8; 16],
    real: &[u8; 16],
) -> Result<Vec<u8>> {
    let mut header = [0u8; 12];
    stream.read_exact(&mut header).await?;
    ensure!(matches!(header[0], b'l' | b'B'), "Invalid X11 byte order");
    let read = |offset: usize| {
        if header[0] == b'l' {
            u16::from_le_bytes([header[offset], header[offset + 1]])
        } else {
            u16::from_be_bytes([header[offset], header[offset + 1]])
        }
    };
    ensure!(
        read(2) == 11 && read(4) == 0,
        "Unsupported X11 protocol version"
    );
    ensure!(
        read(6) == 18 && read(8) == 16,
        "Only MIT-MAGIC-COOKIE-1 authentication is accepted"
    );
    let mut body = [0; 36];
    stream.read_exact(&mut body).await?;
    ensure!(
        &body[..18] == b"MIT-MAGIC-COOKIE-1",
        "Unsupported X11 authentication protocol"
    );
    let difference = body[20..]
        .iter()
        .zip(fake)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b));
    ensure!(difference == 0, "Invalid X11 forwarding cookie");
    body[20..].copy_from_slice(real);
    let mut output = header.to_vec();
    output.extend_from_slice(&body);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(big: bool, cookie: &[u8; 16]) -> Vec<u8> {
        let mut packet = vec![if big { b'B' } else { b'l' }, 0];
        for value in [11u16, 0, 18, 16, 0] {
            packet.extend_from_slice(&if big {
                value.to_be_bytes()
            } else {
                value.to_le_bytes()
            });
        }
        packet.extend_from_slice(b"MIT-MAGIC-COOKIE-1\0\0");
        packet.extend_from_slice(cookie);
        packet
    }

    #[tokio::test]
    async fn authenticates_both_byte_orders_and_replaces_only_cookie() {
        for big in [false, true] {
            let packet = setup(big, &[7; 16]);
            let result = validated_setup(&mut packet.as_slice(), &[7; 16], &[9; 16])
                .await
                .unwrap();
            assert_eq!(&result[..32], &packet[..32]);
            assert_eq!(&result[32..], &[9; 16]);
        }
    }

    #[tokio::test]
    async fn rejects_invalid_cookie_protocol_and_truncated_setup() {
        let valid = setup(false, &[7; 16]);
        let mut invalid_cookie = valid.clone();
        invalid_cookie[47] ^= 1;
        let mut invalid_order = valid.clone();
        invalid_order[0] = b'?';
        let mut invalid_version = valid.clone();
        invalid_version[2] = 10;
        let mut invalid_size = valid.clone();
        invalid_size[7] = 255;
        let mut invalid_name = valid.clone();
        invalid_name[12] = b'?';
        let truncated = valid[..40].to_vec();
        for packet in [
            invalid_cookie,
            invalid_order,
            invalid_version,
            invalid_size,
            invalid_name,
            truncated,
        ] {
            assert!(
                validated_setup(&mut packet.as_slice(), &[7; 16], &[9; 16])
                    .await
                    .is_err()
            );
        }
    }

    #[test]
    fn authority_encodes_display_and_mit_cookie() {
        let bytes = authority(123, &[7; 16]);
        assert_eq!(&bytes[..2], &65535u16.to_be_bytes());
        assert_eq!(&bytes[4..9], b"\0\x03123");
        assert_eq!(&bytes[11..29], b"MIT-MAGIC-COOKIE-1");
        assert_eq!(&bytes[31..], &[7; 16]);
    }

    #[tokio::test]
    async fn shutdown_is_idempotent_and_prevents_startup() {
        let service = WayxService::default();
        service.shutdown();
        service.shutdown();
        assert!(service.acquire().await.is_err());
        assert!(service.lock().server.is_none());
    }

    #[tokio::test]
    #[ignore = "requires a host display and bundled WayX runtime"]
    async fn shared_wayx_reuses_server_and_is_reaped_on_shutdown() {
        let service = WayxService::default();
        let (first, second) = futures::join!(service.acquire(), service.acquire());
        let first = first.expect("start bundled WayX");
        let second = second.expect("reuse bundled WayX");
        assert_eq!(first.display, second.display);
        assert!(first.real == second.real);
        assert!(first.fake != second.fake);
        assert!(Arc::ptr_eq(&first.permits, &second.permits));
        let (directory, pid) = {
            let state = service.lock();
            let server = state.server.as_ref().unwrap();
            (
                server.directory.clone(),
                server.child.as_ref().unwrap().id(),
            )
        };
        let display = first.display;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(directory.join("Xauthority"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        drop(first);
        drop(second);
        // The service survives the last SSH session, and a new session reuses it.
        let third = service.acquire().await.unwrap();
        assert!(third.ready().await);
        assert_eq!(
            service
                .lock()
                .server
                .as_ref()
                .unwrap()
                .child
                .as_ref()
                .unwrap()
                .id(),
            pid
        );
        service.shutdown();
        assert!(!directory.exists());
        assert!(!third.ready().await);
        assert!(service.acquire().await.is_err());
        #[cfg(unix)]
        {
            assert!(!Path::new(&format!("/tmp/.X{display}-lock")).exists());
            assert!(!Path::new(&format!("/tmp/.X11-unix/X{display}")).exists());
        }
        #[cfg(target_os = "linux")]
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
        #[cfg(not(target_os = "linux"))]
        let _ = (display, pid);
    }

    #[tokio::test]
    #[ignore = "requires a host display and bundled WayX runtime"]
    async fn shared_wayx_restarts_after_a_crash() {
        let service = WayxService::default();
        let first = service.acquire().await.unwrap();
        let previous_pid = {
            let mut state = service.lock();
            let child = state.server.as_mut().unwrap().child.as_mut().unwrap();
            let pid = child.id();
            child.kill().unwrap();
            child.wait().unwrap();
            pid
        };
        let replacement = service.acquire().await.unwrap();
        assert!(replacement.ready().await);
        assert!(replacement.real != first.real);
        assert_ne!(
            service
                .lock()
                .server
                .as_ref()
                .unwrap()
                .child
                .as_ref()
                .unwrap()
                .id(),
            previous_pid
        );
        service.shutdown();
    }

    #[tokio::test]
    #[ignore = "requires a host display and bundled WayX runtime"]
    async fn shutdown_during_startup_reaps_the_published_process() {
        let service = WayxService::default();
        let mut startup = Box::pin(service.acquire());
        assert!(futures::poll!(&mut startup).is_pending());
        let (directory, pid) = {
            let state = service.lock();
            let server = state
                .server
                .as_ref()
                .expect("ownership is published before waiting");
            (
                server.directory.clone(),
                server.child.as_ref().unwrap().id(),
            )
        };
        service.shutdown();
        assert!(startup.await.is_err());
        assert!(!directory.exists());
        #[cfg(target_os = "linux")]
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
        #[cfg(not(target_os = "linux"))]
        let _ = pid;
    }
}
