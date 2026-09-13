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
//! - 主机密钥校验按 [`StrictHostKeyChecking`] 的策略走，默认 `Ask`：首次连接 /
//!   密钥变更时弹窗显示指纹并等用户回答（见 `docs/ssh-remote-terminal.md` §3.1）。

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
use parking_lot::Mutex;
use russh::{
    ChannelMsg, ChannelReadHalf, ChannelWriteHalf, Disconnect,
    client::{self},
    keys::{
        HashAlg, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate, check_known_hosts_path,
        known_hosts::{known_host_keys_path, learn_known_hosts_path}, load_secret_key,
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
    /// 主机密钥校验策略，默认 [`StrictHostKeyChecking::Ask`]（弹窗询问）。
    host_key_checking: StrictHostKeyChecking,
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
            host_key_checking: StrictHostKeyChecking::default(),
        }
    }

    /// 指定主机密钥校验策略，对应 OpenSSH 的 `StrictHostKeyChecking`。
    pub fn with_host_key_checking(mut self, policy: StrictHostKeyChecking) -> Self {
        self.host_key_checking = policy;
        self
    }

    pub fn host_key_checking(&self) -> StrictHostKeyChecking {
        self.host_key_checking
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
            .field("host_key_checking", &self.host_key_checking)
            .finish()
    }
}

/// 主机密钥校验策略，语义对齐 OpenSSH 的 `StrictHostKeyChecking`。
///
/// 与 OpenSSH 的唯一差别：`Ask` 遇到**密钥变更**时也允许用户选择替换记录（见
/// [`HostKeyState::Changed`]），而 OpenSSH 的 `ask` 只能停下、让人手工改 known_hosts。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StrictHostKeyChecking {
    /// 未知主机 / 密钥变更时弹窗询问（对应 `ask`，默认值）。
    #[default]
    Ask,
    /// 未知主机直接信任并写入 known_hosts；密钥变更则拒绝（对应 `accept-new`）。
    AcceptNew,
    /// 只接受 known_hosts 中已记录且一致的主机，未知主机直接拒绝（对应 `yes`）。
    Yes,
    /// 一律接受：未知主机写入记录，密钥变更时替换旧记录（对应 `no`）。⚠️ 不安全。
    No,
}

impl StrictHostKeyChecking {
    /// 该策略是否需要弹窗询问用户。
    pub fn asks(self) -> bool {
        matches!(self, Self::Ask)
    }
}

/// 用户对一次主机密钥询问的回答。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostKeyDecision {
    /// 信任并继续：首次连接写入 known_hosts，密钥变更时替换旧记录。
    Trust,
    /// 拒绝连接。
    Reject,
}

/// 这次主机密钥询问的原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostKeyState {
    /// known_hosts 里没有这台主机（首次连接）。
    Unknown,
    /// 有记录但与服务器上报的不一致：主机重装过，或者有人在中间冒充。
    Changed {
        /// 已记录（旧）密钥的 SHA256 指纹。
        known: Vec<String>,
    },
}

/// 一次主机密钥确认请求（SSH 线程 → UI）。
///
/// UI 侧拿到后弹「是 / 否」对话框，把结果交给 [`HostKeyPrompt::respond`]；
/// 重复回答是安全的——只有第一次生效（发送端被取走即空操作）。
#[derive(Clone)]
pub struct HostKeyPrompt {
    user: String,
    host: String,
    port: u16,
    key_type: String,
    fingerprint: String,
    state: HostKeyState,
    responses: Arc<Mutex<Option<UnboundedSender<HostKeyDecision>>>>,
}

impl HostKeyPrompt {
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

    /// 服务器上报的密钥算法，如 `ssh-ed25519`。
    pub fn key_type(&self) -> &str {
        &self.key_type
    }

    /// 服务器上报密钥的 SHA256 指纹（`SHA256:…`）。
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// 这次询问的原因（首次连接 / 密钥变更）。
    pub fn state(&self) -> &HostKeyState {
        &self.state
    }

    /// 回答问题；只有第一次调用生效，之后是空操作。
    pub fn respond(&self, decision: HostKeyDecision) {
        let sender = self.responses.lock().take();
        if let Some(sender) = sender {
            let _ = sender.unbounded_send(decision);
        }
    }
}

impl std::fmt::Debug for HostKeyPrompt {
    /// 手写 `Debug`：`responses` 没有有意义的表示。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostKeyPrompt")
            .field("endpoint", &self.endpoint())
            .field("key_type", &self.key_type)
            .field("fingerprint", &self.fingerprint)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl PartialEq for HostKeyPrompt {
    /// 相等 = 同一次询问（`Event` 要求 `Eq`）。
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.responses, &other.responses)
    }
}

impl Eq for HostKeyPrompt {}

/// 建连超时（TCP + 密钥交换 + 认证）。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// 弹窗询问主机密钥时，等待用户回答的最长时间。
///
/// ⚠️ 这段等待发生在 `client::connect` **内部**（russh 的主机密钥回调就在握手路径上），
/// 所以外层超时要把建连超时再加上它——否则用户还在比指纹，连接就已经超时了。
/// 反过来也不能无限等：对话框可能被无视，这个会话线程会一直挂着。
const HOST_KEY_TIMEOUT: Duration = Duration::from_secs(120);

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
    // 用 Ask 策略时要把「用户看指纹的时间」计进超时（见 `HOST_KEY_TIMEOUT`）。
    let connect_timeout = CONNECT_TIMEOUT
        + if options.host_key_checking().asks() {
            HOST_KEY_TIMEOUT
        } else {
            Duration::ZERO
        };
    let handler = ClientHandler {
        user: options.user().to_string(),
        host: options.host().to_string(),
        port: options.port(),
        known_hosts: options.known_hosts_path(),
        policy: options.host_key_checking(),
        events_tx: events_tx.clone(),
    };
    let mut handle = tokio::time::timeout(
        connect_timeout,
        client::connect(config, (options.host(), options.port()), handler),
    )
    .await
    .map_err(|_| anyhow!("连接超时（{} 秒）", connect_timeout.as_secs()))?
    // `UnknownKey` = 主机密钥没被接受（用户拒绝 / 策略不允许 / 记录不一致）。
    // 直接翻成中文，别让用户对着 “Unknown server key” 猜发生了什么。
    .map_err(|error| match error {
        russh::Error::UnknownKey => anyhow!("主机密钥未被信任，已放弃连接"),
        error => anyhow!(error).context("无法建立 SSH 连接"),
    })?;

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
    user: String,
    host: String,
    port: u16,
    known_hosts: Option<PathBuf>,
    policy: StrictHostKeyChecking,
    /// 需要询问用户时，把 [`HostKeyPrompt`] 送到 UI 的通道。
    events_tx: UnboundedSender<TerminalBackendEvent>,
}

impl ClientHandler {
    /// 展示用端点（日志 / 对话框标题用）。
    fn endpoint(&self) -> String {
        format!("{}@{}:{}", self.user, self.host, self.port)
    }

    /// 把主机密钥写入 known_hosts（首次连接，或用户选择信任）。
    fn remember(&self, key: &PublicKey, path: &Path) {
        if let Err(error) = learn_known_hosts_path(&self.host, self.port, key, path) {
            log::warn!(
                "ssh: 无法把 {} 的主机密钥写入 {}: {error}",
                self.host,
                path.display()
            );
        }
    }

    /// 用新密钥替换 known_hosts 里的旧记录。
    ///
    /// 删不掉旧记录就干脆拒绝：只抱怨不处理的话，下一次连接仍会被同一条旧记录拦下，
    /// 而用户已经被告知「记录会被替换」了。
    fn replace(&self, key: &PublicKey, path: &Path) -> Result<bool, russh::Error> {
        match forget_known_host(&self.host, self.port, path) {
            Ok(removed) if removed > 0 => {
                self.remember(key, path);
                Ok(true)
            }
            Ok(_) => {
                log::warn!(
                    "ssh: {} 的旧记录没有可删的行（哈希形式的 known_hosts 条目无法识别），\
                     请手动清理 {} 后重试",
                    self.endpoint(),
                    path.display()
                );
                Ok(false)
            }
            Err(error) => {
                log::error!(
                    "ssh: 无法删除 {} 在 known_hosts 中的旧记录: {error:#}",
                    self.endpoint()
                );
                Ok(false)
            }
        }
    }

    /// 弹窗询问用户是否信任这台主机，并等待回答。
    ///
    /// 等不到回答一律当作拒绝：
    /// - 对话框被关掉 / 应用退出 → 发送端被丢弃 → 接收端读到 `None`；
    /// - 超过 [`HOST_KEY_TIMEOUT`] → 超时（否则这个会话线程会永远挂着）。
    async fn confirm(
        &self,
        state: HostKeyState,
        key: &PublicKey,
        path: &Path,
    ) -> Result<bool, russh::Error> {
        // 变更过的记录要「替换」而不是「追加」：旧记录留着的话，下次校验仍会报不一致。
        let replace = matches!(state, HostKeyState::Changed { .. });
        let (responses, mut answers) = unbounded();
        let prompt = HostKeyPrompt {
            user: self.user.clone(),
            host: self.host.clone(),
            port: self.port,
            key_type: key.algorithm().to_string(),
            fingerprint: fingerprint(key),
            state,
            responses: Arc::new(Mutex::new(Some(responses))),
        };
        let _ = self
            .events_tx
            .unbounded_send(TerminalBackendEvent::HostKeyPrompt(prompt));

        match tokio::time::timeout(HOST_KEY_TIMEOUT, answers.next()).await {
            Ok(Some(HostKeyDecision::Trust)) => {
                log::info!("ssh: 用户信任了 {} 的主机密钥", self.endpoint());
                if replace {
                    self.replace(key, path)
                } else {
                    self.remember(key, path);
                    Ok(true)
                }
            }
            Ok(Some(HostKeyDecision::Reject)) => {
                log::info!("ssh: 用户拒绝信任 {} 的主机密钥", self.endpoint());
                Ok(false)
            }
            Ok(None) => {
                log::warn!("ssh: {} 的主机密钥确认无人回答，按拒绝处理", self.endpoint());
                Ok(false)
            }
            Err(_) => {
                log::warn!(
                    "ssh: 等待 {} 的主机密钥确认超过 {} 秒，按拒绝处理",
                    self.endpoint(),
                    HOST_KEY_TIMEOUT.as_secs()
                );
                Ok(false)
            }
        }
    }
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    /// 主机密钥校验，按 [`StrictHostKeyChecking`] 处理三种结果：
    /// - 已记录且一致 → 接受；
    /// - 没有记录（首次连接）→ 策略决定（询问 / 写入后接受 / 拒绝）；
    /// - 与记录不符（主机重装，或可能被中间人替换）→ 策略决定（询问 / 替换 / 拒绝）。
    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // `public_key()` 按值返回；后面要带着它 await，所以要个所有权。
        let key = server_public_key.public_key();

        // 定位不到文件（没有 home 目录）时只能接受：与 OpenSSH「没有 known_hosts
        // 就当作首次连接」的行为一致，总比完全连不上好。
        let Some(path) = self.known_hosts.clone() else {
            log::warn!("ssh: 定位不到 known_hosts，跳过 {} 的主机密钥校验", self.host);
            return Ok(true);
        };

        match check_known_hosts_path(&self.host, self.port, &key, &path) {
            Ok(true) => Ok(true),
            Ok(false) => match self.policy {
                StrictHostKeyChecking::AcceptNew | StrictHostKeyChecking::No => {
                    self.remember(&key, &path);
                    Ok(true)
                }
                StrictHostKeyChecking::Yes => {
                    log::warn!(
                        "ssh: {} 不在 known_hosts 中，策略为 yes → 拒绝",
                        self.endpoint()
                    );
                    Ok(false)
                }
                StrictHostKeyChecking::Ask => {
                    log::info!("ssh: {} 首次连接，等待用户确认主机密钥", self.endpoint());
                    self.confirm(HostKeyState::Unknown, &key, &path).await
                }
            },
            Err(error) => match self.policy {
                StrictHostKeyChecking::No => {
                    log::warn!(
                        "ssh: {} 的主机密钥已变更（{error}），策略为 no → 直接替换",
                        self.endpoint()
                    );
                    self.replace(&key, &path)
                }
                StrictHostKeyChecking::AcceptNew | StrictHostKeyChecking::Yes => {
                    log::error!(
                        "ssh: {} 的主机密钥与 known_hosts 记录不一致，已拒绝连接: {error}",
                        self.endpoint()
                    );
                    Ok(false)
                }
                StrictHostKeyChecking::Ask => {
                    // 旧记录只用于在对话框里展示对比，读不到就把列表留空。
                    let known = known_host_keys_path(&self.host, self.port, &path)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(_, recorded)| fingerprint(&recorded))
                        .collect();
                    log::warn!("ssh: {} 的主机密钥已变更，等待用户确认", self.endpoint());
                    self.confirm(HostKeyState::Changed { known }, &key, &path)
                        .await
                }
            },
        }
    }
}

/// OpenSSH 风格的 SHA256 指纹（`SHA256:…`）。
fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

/// 把 `host:port` 在 known_hosts 中的所有记录行删掉，返回删除的行数。
///
/// 主机名按 OpenSSH 的存储形式匹配：22 端口写 `host`，其余写 `[host]:port`；
/// 一行的主机字段可以是逗号分隔的多个名字。同一主机的**所有**记录行都会被删掉
/// （含其它算法的那几条）——用户既然选择「替换」，旧记录就都不可信了。
///
/// ⚠️ 只认字面名字：`HashKnownHosts` 写出的 `|1|salt|hash` 条目（哈希后无法回推）
/// 匹配不到，此时返回 0，调用方会拒绝连接并让用户手工清理。
fn forget_known_host(host: &str, port: u16, path: &Path) -> anyhow::Result<usize> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("无法读取 {}", path.display()))?;
    let target = if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    };

    let mut removed = 0;
    let mut kept: Vec<&str> = Vec::new();
    for line in content.lines() {
        let matched = !line.starts_with('#')
            && line
                .split_whitespace()
                .next()
                .is_some_and(|names| names.split(',').any(|name| name == target));
        if matched {
            removed += 1;
        } else {
            kept.push(line);
        }
    }

    if removed > 0 {
        let mut rewritten = kept.join("\n");
        if !rewritten.is_empty() {
            rewritten.push('\n');
        }
        std::fs::write(path, rewritten).with_context(|| format!("无法写入 {}", path.display()))?;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt as _;
    use russh::client::Handler as _;

    /// `localhost` 的 ed25519 公钥（取自 russh 自己的 known_hosts 测试）。
    const LOCALHOST_KEY: &str =
        "AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";

    /// 建一个临时 known_hosts（没有 tempfile 依赖，用临时目录 + pid 命名）。
    ///
    /// `name` 要每个用例各不相同：cargo 默认并行跑测试，共用文件名会互相覆盖。
    fn temp_known_hosts(name: &str, content: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("alacrterm-ssh-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    /// 替换旧记录：只删这台主机这一端口的行，别的主机与端口不同的行要留着。
    #[test]
    fn forget_known_host_only_removes_that_endpoint() {
        let path = temp_known_hosts(
            "replace-endpoint",
            &format!(
                "# comment\nlocalhost ssh-ed25519 {LOCALHOST_KEY}\n\
                 [localhost]:13265 ssh-ed25519 {LOCALHOST_KEY}\n\
                 other.example ssh-ed25519 {LOCALHOST_KEY}\n"
            ),
        );

        let removed = forget_known_host("localhost", 13265, &path).unwrap();
        assert_eq!(removed, 1);

        let left = std::fs::read_to_string(&path).unwrap();
        assert!(left.contains("# comment"), "注释行不该动: {left}");
        assert!(
            left.lines().any(|line| line.starts_with("localhost ")),
            "22 端口的记录该留着: {left}"
        );
        assert!(
            left.lines().any(|line| line.starts_with("other.example ")),
            "别的主机该留着: {left}"
        );
        assert!(!left.contains("[localhost]:13265"), "被替换的记录该删掉: {left}");

        std::fs::remove_file(&path).ok();
    }

    /// 逗号分隔的主机名也认（`host,ip` 是 OpenSSH 常见的写法）。
    #[test]
    fn forget_known_host_matches_comma_separated_names() {
        let path = temp_known_hosts(
            "comma-names",
            &format!(
                "localhost,127.0.0.1 ssh-ed25519 {LOCALHOST_KEY}\n\
                 other.example ssh-ed25519 {LOCALHOST_KEY}\n"
            ),
        );

        assert_eq!(forget_known_host("127.0.0.1", 22, &path).unwrap(), 1);

        let left = std::fs::read_to_string(&path).unwrap();
        assert!(!left.contains("127.0.0.1"), "这一行该删掉: {left}");
        assert!(left.contains("other.example"), "别的主机该留着: {left}");

        std::fs::remove_file(&path).ok();
    }

    /// 哈希形式的条目（`HashKnownHosts`）认不出来：返回 0，调用方据此拒绝连接
    /// 而不是假装自己删掉了旧记录。
    #[test]
    fn forget_known_host_does_not_touch_hashed_entries() {
        let hashed = "|1|O33ESRMWPVkMYIwJ1Uw+n877jTo=|nuuC5vEqXlEZ/8BXQR7m619W6Ak=";
        let content = format!("{hashed} ssh-ed25519 {LOCALHOST_KEY}\n");
        let path = temp_known_hosts("hashed", &content);

        assert_eq!(forget_known_host("localhost", 22, &path).unwrap(), 0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);

        std::fs::remove_file(&path).ok();
    }

    /// 没有记录时什么都不改（不该把文件清空或报错）。
    #[test]
    fn forget_known_host_is_noop_without_record() {
        let content = format!("other.example ssh-ed25519 {LOCALHOST_KEY}\n");
        let path = temp_known_hosts("noop", &content);

        assert_eq!(forget_known_host("localhost", 22, &path).unwrap(), 0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);

        std::fs::remove_file(&path).ok();
    }

    /// 另一把 ed25519 公钥（russh 测试里那把「变更后」的密钥），用来构造不一致的场景。
    const OTHER_KEY: &str =
        "AAAAC3NzaC1lZDI1NTE5AAAAILIG2T/B0l0gaqj3puu510tu9N1OkQ4znY3LYuEm5zCF";

    /// 服务器上报的主机密钥（校验回调的入参）。
    fn server_key(base64: &str) -> PublicKeyOrCertificate {
        PublicKeyOrCertificate::PublicKey {
            key: PublicKey::from_openssh(&format!("ssh-ed25519 {base64}")).unwrap(),
            hash_alg: None,
        }
    }

    /// 造一个 handler：`known_hosts` 指向临时文件，事件通道的另一端交回给测试。
    fn handler(
        policy: StrictHostKeyChecking,
        path: &Path,
    ) -> (ClientHandler, UnboundedReceiver<TerminalBackendEvent>) {
        let (events_tx, events_rx) = unbounded();
        let handler = ClientHandler {
            user: "tester".to_string(),
            host: "localhost".to_string(),
            port: 22,
            known_hosts: Some(path.to_path_buf()),
            policy,
            events_tx,
        };
        (handler, events_rx)
    }

    /// 驱动一次 `check_server_key`；`answer` 为 `Some` 时等第一个询问并照它回答。
    ///
    /// 返回（是否接受，收到的询问）。整体带超时：万一该问而没问，测试会失败而不是挂死。
    fn check(
        handler: &mut ClientHandler,
        events_rx: &mut UnboundedReceiver<TerminalBackendEvent>,
        key: &PublicKeyOrCertificate,
        answer: Option<HostKeyDecision>,
    ) -> (bool, Option<HostKeyPrompt>) {
        let mut seen = None;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let accepted = runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                let (accepted, ()) = futures::join!(
                    handler.check_server_key(key),
                    async {
                        if let Some(answer) = answer {
                            match events_rx.next().await {
                                Some(TerminalBackendEvent::HostKeyPrompt(prompt)) => {
                                    prompt.respond(answer);
                                    seen = Some(prompt);
                                }
                                other => panic!("期望主机密钥询问，实际收到 {other:?}"),
                            }
                        }
                    }
                );
                accepted.unwrap()
            })
            .await
            .expect("等待主机密钥校验超时（可能在等一个永远不会来的询问）")
        });
        (accepted, seen)
    }

    /// 首次连接 + `Ask`：弹窗询问；用户选「信任」→ 写入 known_hosts。
    #[test]
    fn ask_policy_prompts_and_remembers_on_trust() {
        let path = temp_known_hosts("ask-trust", "");
        let (mut handler, mut events_rx) = handler(StrictHostKeyChecking::Ask, &path);
        let key = server_key(LOCALHOST_KEY);

        let (accepted, prompt) = check(&mut handler, &mut events_rx, &key, Some(HostKeyDecision::Trust));

        assert!(accepted, "用户选择信任后应当继续连接");
        let prompt = prompt.expect("首次连接必须先询问");
        assert_eq!(prompt.endpoint(), "tester@localhost:22");
        assert_eq!(prompt.key_type(), "ssh-ed25519");
        assert!(prompt.fingerprint().starts_with("SHA256:"), "{}", prompt.fingerprint());
        assert_eq!(*prompt.state(), HostKeyState::Unknown);
        assert!(
            std::fs::read_to_string(&path).unwrap().contains(LOCALHOST_KEY),
            "信任后应当把密钥记进 known_hosts"
        );

        std::fs::remove_file(&path).ok();
    }

    /// 首次连接 + `Ask`：用户选「取消」→ 拒绝连接，且不写 known_hosts。
    #[test]
    fn ask_policy_rejects_on_decline() {
        let path = temp_known_hosts("ask-reject", "");
        let (mut handler, mut events_rx) = handler(StrictHostKeyChecking::Ask, &path);
        let key = server_key(LOCALHOST_KEY);

        let (accepted, prompt) = check(&mut handler, &mut events_rx, &key, Some(HostKeyDecision::Reject));

        assert!(!accepted, "用户取消后不能连接");
        assert!(prompt.is_some(), "应当先询问");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "", "拒绝后不该留下记录");

        std::fs::remove_file(&path).ok();
    }

    /// `AcceptNew` 不询问：直接信任并记下来（= 本次改动之前的行为）。
    #[test]
    fn accept_new_policy_never_prompts() {
        let path = temp_known_hosts("accept-new", "");
        let (mut handler, mut events_rx) = handler(StrictHostKeyChecking::AcceptNew, &path);
        let key = server_key(LOCALHOST_KEY);

        let (accepted, prompt) = check(&mut handler, &mut events_rx, &key, None);

        assert!(accepted);
        assert!(prompt.is_none(), "AcceptNew 不该弹窗");
        assert!(std::fs::read_to_string(&path).unwrap().contains(LOCALHOST_KEY));

        std::fs::remove_file(&path).ok();
    }

    /// `Yes` 只认已记录的主机：未知主机直接拒绝，且不写记录。
    #[test]
    fn yes_policy_rejects_unknown_host() {
        let path = temp_known_hosts("yes-unknown", "");
        let (mut handler, mut events_rx) = handler(StrictHostKeyChecking::Yes, &path);
        let key = server_key(LOCALHOST_KEY);

        let (accepted, prompt) = check(&mut handler, &mut events_rx, &key, None);

        assert!(!accepted, "yes 策略下未知主机必须拒绝");
        assert!(prompt.is_none());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");

        std::fs::remove_file(&path).ok();
    }

    /// 密钥与记录不一致 + `Ask`：询问时带上旧指纹；用户选「信任」→ 用新密钥替换旧记录，
    /// 其它主机的记录不受影响。
    #[test]
    fn ask_policy_replaces_changed_key_on_trust() {
        let path = temp_known_hosts(
            "ask-changed",
            &format!("localhost ssh-ed25519 {OTHER_KEY}\nother.example ssh-ed25519 {OTHER_KEY}\n"),
        );
        let (mut handler, mut events_rx) = handler(StrictHostKeyChecking::Ask, &path);
        let key = server_key(LOCALHOST_KEY);

        let (accepted, prompt) = check(&mut handler, &mut events_rx, &key, Some(HostKeyDecision::Trust));

        assert!(accepted);
        let prompt = prompt.expect("密钥变更必须先询问");
        match prompt.state() {
            HostKeyState::Changed { known } => {
                assert_eq!(known.len(), 1, "应当列出旧指纹: {known:?}");
                assert!(known[0].starts_with("SHA256:"), "{known:?}");
            }
            other => panic!("应当是 Changed，实际 {other:?}"),
        }

        let left = std::fs::read_to_string(&path).unwrap();
        assert!(
            left.lines()
                .any(|line| line.starts_with("localhost ") && line.contains(LOCALHOST_KEY)),
            "localhost 的记录该换成新密钥: {left}"
        );
        assert!(
            left.lines()
                .any(|line| line.starts_with("other.example ") && line.contains(OTHER_KEY)),
            "别的主机的记录不该受影响: {left}"
        );
        // 旧记录被删干净 + 新记录写入 ⇒ 下一次连接会走「已记录且一致」这条快路。
        assert_eq!(forget_known_host("localhost", 22, &path).unwrap(), 1);

        std::fs::remove_file(&path).ok();
    }

    /// 密钥与记录不一致 + `AcceptNew`（非 Ask）：不做任何询问，直接拒绝。
    #[test]
    fn accept_new_policy_rejects_changed_key() {
        let path = temp_known_hosts(
            "accept-new-changed",
            &format!("localhost ssh-ed25519 {OTHER_KEY}\n"),
        );
        let (mut handler, mut events_rx) = handler(StrictHostKeyChecking::AcceptNew, &path);
        let key = server_key(LOCALHOST_KEY);

        let (accepted, prompt) = check(&mut handler, &mut events_rx, &key, None);

        assert!(!accepted, "密钥变更且不询问时必须拒绝");
        assert!(prompt.is_none(), "AcceptNew 不该弹窗");
        assert!(
            std::fs::read_to_string(&path).unwrap().contains(OTHER_KEY),
            "拒绝时不该改动 known_hosts"
        );

        std::fs::remove_file(&path).ok();
    }
}
