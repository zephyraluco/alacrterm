//! SSH 远端终端后端：用 `russh` 直连远端主机（不再依赖系统 OpenSSH 客户端）。
//!
//! 与本地 PTY 后端的差别只在「数据从哪来、往哪去」：
//! - **下行**：终端视图的按键 / 尺寸变化 / 关闭请求经 [`SshInput`] 送进专属线程，
//!   由它写进 SSH 通道（`Channel::data_bytes` / `window_change` / `eof`）；
//! - **上行**：远端 shell 的输出由 `Channel::wait` 读出，直接喂给 `alacritty_terminal`
//!   的 `Processor`，再发一个 `Wakeup` 让 UI 重绘（和本地 PTY 的事件循环同一套语义）。
//!
//! 因此终端仿真、渲染、输入法、选择等上层逻辑**完全不知道**自己在跑远端会话。
//!
//! ## 并发模型
//!
//! 连接、认证与 IO 全部跑在**一个专属 OS 线程**上（`alacrterm-ssh`），
//! 该线程里建一个 current-thread 的 tokio 运行时——russh 要求 tokio 的
//! `AsyncRead`/`AsyncWrite`，而 gpui 的后台执行器不是 tokio 运行时。
//! 线程通过 `futures` 的无界通道与 gpui 侧通信（两侧都是纯发送，不需要运行时互通）。
//!
//! ## 已知边界（有意未做）
//!
//! - 不解析 `~/.ssh/config`（Host 别名 / ProxyJump / IdentityFile / 自定义端口）；
//! - 不支持加密私钥（需要口令输入，当前界面没有这个入口）与 ssh-agent 转发；
//! - 主机密钥是 **TOFU**（首次连接直接信任并写入 `known_hosts`，之后不一致则拒绝）。

use std::{
    borrow::Cow,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use alacritty_terminal::{
    event::WindowSize,
    vte::ansi::{Processor, StdSyncHandler},
};
use anyhow::{Context as _, anyhow, bail};
use futures::{
    StreamExt as _,
    channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded},
};
use russh::{
    ChannelMsg, ChannelReadHalf, ChannelWriteHalf, Disconnect,
    client::{self},
    keys::{
        PrivateKeyWithHashAlg, PublicKeyOrCertificate, check_known_hosts_path,
        known_hosts::learn_known_hosts_path, load_secret_key,
    },
};

use crate::{
    TerminalBackendEvent,
    alacritty::{AlacrittyTermLock, TerminalInput},
};

/// 建立 SSH 连接所需的参数（由「新建终端」对话框收集）。
#[derive(Clone)]
pub struct SshOptions {
    host: String,
    port: u16,
    user: String,
    password: Option<String>,
    /// 主机密钥记录文件；`None` = `~/.ssh/known_hosts`。
    known_hosts: Option<PathBuf>,
}

impl SshOptions {
    pub fn new(
        host: impl Into<String>,
        port: u16,
        user: impl Into<String>,
        password: Option<String>,
    ) -> Self {
        Self {
            host: host.into(),
            port,
            user: user.into(),
            password,
            known_hosts: None,
        }
    }

    /// 指定主机密钥记录文件，对应 OpenSSH 的 `UserKnownHostsFile`。
    ///
    /// 不设置就用默认位置 `~/.ssh/known_hosts`；测试用它避免污染真实用户的文件。
    pub fn with_known_hosts(mut self, path: impl Into<PathBuf>) -> Self {
        self.known_hosts = Some(path.into());
        self
    }

    /// 展示用端点（`user@host:port`）。
    pub fn endpoint(&self) -> String {
        format!("{}@{}:{}", self.user, self.host, self.port)
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn user(&self) -> &str {
        &self.user
    }

    pub fn password(&self) -> Option<&str> {
        self.password.as_deref()
    }

    /// 主机密钥记录文件的实际位置；取不到 home 目录时为 `None`。
    fn known_hosts_path(&self) -> Option<PathBuf> {
        self.known_hosts
            .clone()
            .or_else(|| dirs::home_dir().map(|home| home.join(".ssh").join("known_hosts")))
    }
}

impl std::fmt::Debug for SshOptions {
    /// 手写 `Debug`：`derive` 会把密码原文写进日志。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshOptions")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("password", &self.password.as_ref().map(|_| "<已设置>"))
            .finish()
    }
}

/// 建连超时（TCP + 密钥交换 + 主机密钥校验）。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// 保活间隔：定期向远端发 keepalive，让「网线被拔」这类静默断开最终能报错。
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);

/// `request_pty` 时上报的初始窗口尺寸。
///
/// 不能直接用 `TerminalBounds::default()`（100 列 × 6 行）：终端实体在任何一次布局
/// 之前就是这个尺寸，拿它开远端 PTY 会让 shell 先看到一个 6 行的窗口——某些 shell /
/// readline 在极矮窗口下会重排甚至清屏。这里先按惯例报 80×24，紧接着的第一次布局
/// 就会用真实的窗口尺寸 `window_change` 覆盖它。
const INITIAL_PTY_SIZE: (u16, u16) = (80, 24);

/// 下行命令（终端视图 → SSH 线程）。
enum Command {
    Write(Vec<u8>),
    Resize(WindowSize),
    Shutdown,
}

/// SSH 后端的下行句柄：由 `Terminal` 持有，写入用户输入、尺寸变化与关闭请求。
#[derive(Clone)]
pub(super) struct SshInput {
    commands: UnboundedSender<Command>,
}

impl TerminalInput for SshInput {
    fn write(&self, input: Cow<'static, [u8]>) {
        let _ = self.commands.unbounded_send(Command::Write(input.into_owned()));
    }

    fn resize(&self, size: WindowSize) {
        let _ = self.commands.unbounded_send(Command::Resize(size));
    }

    fn shutdown(&self) {
        let _ = self.commands.unbounded_send(Command::Shutdown);
    }
}

/// 启动一个 SSH 会话线程，立即返回它的下行句柄。
///
/// 连接是**异步**进行的：连不上不会让终端创建失败，而是把原因作为文本写进终端网格
/// （见 [`write_notice`]），会话随即以「已断开」状态保留在标签页里。
pub(super) fn spawn(
    options: SshOptions,
    term: Arc<AlacrittyTermLock>,
    events_tx: UnboundedSender<TerminalBackendEvent>,
) -> SshInput {
    let (commands, receiver) = unbounded();
    let input = SshInput { commands };

    // 连接、认证与 IO 全在专属线程里跑：russh 要 tokio 的 IO trait，而 gpui 的后台
    // 执行器不是 tokio 运行时，混用会把整个会话绑死在错误的任务上。
    let thread = std::thread::Builder::new()
        .name("alacrterm-ssh".to_string())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            match runtime {
                Ok(runtime) => runtime.block_on(session(options, term, events_tx, receiver)),
                Err(error) => log::error!("ssh: 无法创建 tokio 运行时: {error}"),
            }
        });

    if let Err(error) = thread {
        log::error!("ssh: 无法创建会话线程: {error}");
    }

    input
}

/// 一个 SSH 会话的完整生命周期：建连 → 认证 → 开通道 → 双向转发 → 收尾提示。
async fn session(
    options: SshOptions,
    term: Arc<AlacrittyTermLock>,
    events_tx: UnboundedSender<TerminalBackendEvent>,
    commands: UnboundedReceiver<Command>,
) {
    match run(&options, &term, &events_tx, commands).await {
        Ok(notice) => write_notice(&term, &events_tx, &dim_line(&notice)),
        Err(error) => {
            log::warn!("ssh: 会话异常结束（{}）: {error:#}", options.endpoint());
            write_notice(&term, &events_tx, &red_line(&format!("连接中断：{error:#}")));
        }
    }

    // 通知 UI 会话已结束：状态栏显示「已断开」，而网格内容与标签页都保留
    // （远端报的断开原因不会被丢掉）。
    let _ = events_tx.unbounded_send(TerminalBackendEvent::Exit);
}

/// 建连 + 认证 + 执行远端 shell，返回会话结束时的提示语。
async fn run(
    options: &SshOptions,
    term: &AlacrittyTermLock,
    events_tx: &UnboundedSender<TerminalBackendEvent>,
    mut commands: UnboundedReceiver<Command>,
) -> anyhow::Result<String> {
    // 「正在连接…」不带换行：连上后原地擦掉（`\r\x1b[2K`），连不上则换行写原因。
    write_notice(
        term,
        events_tx,
        &dim(&format!("正在连接 {} …", options.endpoint())),
    );

    let config = Arc::new(client::Config::default());
    let handler = ClientHandler {
        host: options.host.clone(),
        port: options.port,
        known_hosts: options.known_hosts_path(),
    };
    let mut handle = tokio::time::timeout(
        CONNECT_TIMEOUT,
        client::connect(
            config,
            (options.host.as_str(), options.port),
            handler,
        ),
    )
    .await
    .map_err(|_| anyhow!("连接超时（{} 秒）", CONNECT_TIMEOUT.as_secs()))?
    .context("无法建立 SSH 连接")?;

    authenticate(&mut handle, options).await?;

    let channel = handle
        .channel_open_session()
        .await
        .context("远端拒绝打开会话通道")?;
    // 拆成读 / 写两半：读半边可以在 select 里独立等待远端数据，写半边随时可用。
    // （不分拆的话 `Channel::wait` 的 `&mut self` 会和 `data_bytes` 的借用打架。）
    let (mut reader, writer) = channel.split();
    writer
        .request_pty(
            true,
            "xterm-256color",
            u32::from(INITIAL_PTY_SIZE.0),
            u32::from(INITIAL_PTY_SIZE.1),
            0,
            0,
            &[],
        )
        .await
        .context("远端拒绝分配伪终端")?;
    writer
        .request_shell(true)
        .await
        .context("远端拒绝启动 shell")?;

    // 连上了：告诉上层「通道就绪」，并把「正在连接…」那一行擦掉，让位给远端输出。
    let _ = events_tx.unbounded_send(TerminalBackendEvent::Connected);
    write_notice(term, events_tx, "\r\x1b[2K");

    pump(
        term,
        events_tx,
        &mut handle,
        &writer,
        &mut reader,
        &mut commands,
    )
    .await
}

/// 认证：按「密码 → 免密 → 本地私钥」的顺序尝试。
async fn authenticate(
    handle: &mut client::Handle<ClientHandler>,
    options: &SshOptions,
) -> anyhow::Result<()> {
    if let Some(password) = options.password() {
        let result = handle
            .authenticate_password(options.user(), password.to_string())
            .await
            .context("密码认证失败")?;
        if result.success() {
            return Ok(());
        }
        bail!("密码认证被拒绝");
    }

    // 有些服务器（或已配置好的免密环境）允许 none 直接通过。
    if handle
        .authenticate_none(options.user())
        .await
        .context("免密认证失败")?
        .success()
    {
        return Ok(());
    }

    let mut tried = false;
    for path in private_key_paths() {
        if !path.is_file() {
            continue;
        }
        tried = true;
        match authenticate_with_key(handle, options, &path).await {
            Ok(true) => return Ok(()),
            Ok(false) => continue,
            Err(error) => log::debug!("ssh: {} 不可用: {error:#}", path.display()),
        }
    }

    if tried {
        bail!("认证失败：服务器拒绝免密登录，且 ~/.ssh 下的私钥都不被接受");
    }
    bail!("认证失败：未填写密码，且 ~/.ssh 下没有可用的私钥（id_ed25519 / id_ecdsa / id_rsa）");
}

/// 用 `path` 处的私钥认证。返回 `false` 表示服务器不接受这把钥匙（不是致命错误）。
async fn authenticate_with_key(
    handle: &mut client::Handle<ClientHandler>,
    options: &SshOptions,
    path: &Path,
) -> anyhow::Result<bool> {
    // 带口令的私钥需要交互输入口令，当前没有这个入口，直接跳过（`load_secret_key` 会报错）。
    let key = load_secret_key(path, None)
        .with_context(|| format!("无法读取私钥 {}", path.display()))?;
    let key = Arc::new(key);

    // RSA 需要和服务器协商签名哈希；其余算法必须传 `None`。
    let hash_alg = if key.algorithm().is_rsa() {
        handle
            .best_supported_rsa_hash()
            .await
            .ok()
            .flatten()
            .flatten()
    } else {
        None
    };

    let result = handle
        .authenticate_publickey(
            options.user(),
            PrivateKeyWithHashAlg::new(key, hash_alg),
        )
        .await
        .with_context(|| format!("公钥认证失败（{}）", path.display()))?;
    Ok(result.success())
}

/// 认证用私钥的候选位置（与 OpenSSH 的默认顺序一致）。
fn private_key_paths() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    ["id_ed25519", "id_ecdsa", "id_rsa"]
        .iter()
        .map(|name| home.join(".ssh").join(name))
        .collect()
}

/// 双向转发：把用户输入写进通道，把远端输出喂给终端仿真器。
async fn pump(
    term: &AlacrittyTermLock,
    events_tx: &UnboundedSender<TerminalBackendEvent>,
    handle: &mut client::Handle<ClientHandler>,
    writer: &ChannelWriteHalf<client::Msg>,
    reader: &mut ChannelReadHalf,
    commands: &mut UnboundedReceiver<Command>,
) -> anyhow::Result<String> {
    let mut processor = Processor::<StdSyncHandler>::new();
    let mut exit_status: Option<u32> = None;

    let mut keepalive = tokio::time::interval(KEEPALIVE_INTERVAL);
    keepalive.tick().await; // `interval` 的第一次 tick 立即就绪，先消耗掉。

    loop {
        tokio::select! {
            biased;

            command = commands.next() => match command {
                Some(Command::Write(bytes)) => {
                    writer
                        .data_bytes(bytes)
                        .await
                        .context("向远端写入失败")?;
                }
                Some(Command::Resize(size)) => {
                    let _ = writer
                        .window_change(
                            u32::from(size.num_cols),
                            u32::from(size.num_lines),
                            u32::from(size.cell_width),
                            u32::from(size.cell_height),
                        )
                        .await;
                }
                // 标签页关闭 / 应用退出：先给远端一个 EOF，让它体面收尾。
                Some(Command::Shutdown) | None => {
                    let _ = writer.eof().await;
                    let _ = writer.close().await;
                    break;
                }
            },

            message = reader.wait() => match message {
                Some(ChannelMsg::Data { data }) => feed(term, events_tx, &mut processor, &data),
                // stderr（扩展数据流 1）也是终端输出，一并喂给仿真器。
                Some(ChannelMsg::ExtendedData { data, ext }) if ext == 1 => {
                    feed(term, events_tx, &mut processor, &data);
                }
                Some(ChannelMsg::ExitStatus { exit_status: code }) => exit_status = Some(code),
                // EOF / 通道关闭都表示远端 shell 已经走了。
                Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => break,
                Some(_) => {}
            },

            // 保活：让「网线被拔」这类静默断开最终能被发现。
            _ = keepalive.tick() => {
                if handle.is_closed() {
                    break;
                }
                let _ = handle.send_keepalive(true).await;
            }
        }
    }

    let _ = handle.disconnect(Disconnect::ByApplication, "", "en").await;

    Ok(match exit_status {
        Some(code) => format!("连接已断开（远端退出码 {code}）"),
        None => "连接已断开".to_string(),
    })
}

/// 把远端字节喂给终端仿真器，并让 UI 重绘。
fn feed(
    term: &AlacrittyTermLock,
    events_tx: &UnboundedSender<TerminalBackendEvent>,
    processor: &mut Processor<StdSyncHandler>,
    bytes: &[u8],
) {
    processor.advance(&mut *term.lock(), bytes);
    let _ = events_tx.unbounded_send(TerminalBackendEvent::Wakeup);
}

/// 往终端网格里写一行本机提示（连接进度 / 断开原因）。
///
/// SSH 后端没有可写的本地进程，这类提示只能自己生成 ANSI 序列喂给仿真器。
/// 换行一律写 `\r\n`：仿真器默认不把单独的 `\n` 视作回车换行。
fn write_notice(
    term: &AlacrittyTermLock,
    events_tx: &UnboundedSender<TerminalBackendEvent>,
    ansi: &str,
) {
    let mut processor = Processor::<StdSyncHandler>::new();
    processor.advance(&mut *term.lock(), ansi.as_bytes());
    let _ = events_tx.unbounded_send(TerminalBackendEvent::Wakeup);
}

/// 弱化色（灰）文本，用于「正在连接…」这类行内提示（不带换行）。
fn dim(text: &str) -> String {
    format!("\x1b[90m{text}\x1b[0m")
}

/// 弱化色的独立行。
fn dim_line(text: &str) -> String {
    format!("\r\n{}\r\n", dim(text))
}

/// 错误色的独立行。
fn red_line(text: &str) -> String {
    format!("\r\n\x1b[31m{text}\x1b[0m\r\n")
}

/// 客户端回调：只做一件事——校验主机密钥。
struct ClientHandler {
    host: String,
    port: u16,
    known_hosts: Option<PathBuf>,
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    /// 主机密钥校验采用 **TOFU**（首次使用即信任），与 `ssh` 首次连接时回答 `yes` 等价：
    /// - 已记录且一致 → 接受；
    /// - 没有记录（首次）→ 记入 known_hosts 后接受；
    /// - 与记录不符（可能被中间人替换）→ 拒绝。
    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // `public_key()` 按值返回，借出去给 known_hosts 的两个函数用。
        let key = &server_public_key.public_key();

        // 定位不到文件（没有 home 目录）时只能接受：与 OpenSSH「没有 known_hosts
        // 就当作首次连接」的行为一致，总比完全连不上好。
        let Some(path) = &self.known_hosts else {
            log::warn!("ssh: 定位不到 known_hosts，跳过 {} 的主机密钥校验", self.host);
            return Ok(true);
        };

        match check_known_hosts_path(&self.host, self.port, key, path) {
            Ok(true) => Ok(true),
            Ok(false) => {
                if let Err(error) = learn_known_hosts_path(&self.host, self.port, key, path) {
                    log::warn!(
                        "ssh: 无法把 {} 的主机密钥写入 {}: {error}",
                        self.host,
                        path.display()
                    );
                }
                Ok(true)
            }
            Err(error) => {
                log::error!(
                    "ssh: {} 的主机密钥与 known_hosts 记录不一致，已拒绝连接: {error}",
                    self.host
                );
                Ok(false)
            }
        }
    }
}
