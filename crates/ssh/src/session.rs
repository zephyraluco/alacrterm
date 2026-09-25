//! 会话本体:自带 tokio 运行时的 OS 线程 + 命令/事件两条通道。
//!
//! 线程里干这三件事(见 [`run`]):
//! 1. **连接**:自己 dial TCP(而不是用 `russh::client::connect`),这样「连不上哪个
//!    地址」能带上主机与端口报错,而不是一句干巴巴的 IO 错误;
//! 2. **开会话通道**:认证 → `session` 通道 → 申请 PTY → 申请 shell;
//! 3. **双向搬运**:远端 `ChannelMsg::Data` 转成 [`SshEvent::Data`];本地命令
//!    (写键盘 / 改尺寸 / 断开)转成信道消息。
//!
//! ⚠️ 第 3 步必须用 `Channel::split()` 把通道拆成读写两半:写方法(`data_bytes` /
//! `window_change`)要 `&self`、`wait()` 要 `&mut self`,同一个 `Channel` 上放不进一个
//! `tokio::select!`(两支会同时借 `channel`)。

use std::sync::Arc;
use std::time::Duration;

use futures::channel::mpsc::UnboundedSender;
use russh::client;
use russh::{ChannelMsg, Disconnect};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender as TokioSender, unbounded_channel};

use crate::auth;
use crate::error::SshError;
use crate::fs::SshFs;
use crate::handler::{ClientHandler, HostKeyPrompt};
use crate::params::{SshParams, SshSize};

/// 会话向宿主冒出的事件。
#[derive(Debug, Clone)]
pub enum SshEvent {
    /// 进度文字(正在连接 / 正在认证 / 失败原因)。宿主通常直接显示给用户 ——
    /// 建连 + 认证可能要几秒,没有它界面就是「卡住」的样子。
    Status(String),
    /// 远端 PTY 的原样字节(可能包含任何控制序列),交给终端模拟器解析。
    Data(Vec<u8>),
    /// 首次连接某台主机,需要宿主问用户「信任这个指纹吗」。
    ///
    /// 握手线程在等回答(最多 [`HOST_KEY_TIMEOUT`](crate::HOST_KEY_TIMEOUT)):
    /// 宿主弹窗,用户选完调 [`HostKeyPrompt::respond`]。不回答则超时放弃连接。
    HostKeyPrompt(HostKeyPrompt),
    /// 会话可用了(握手 / 认证 / 申请 PTY 与 shell 全部成功)。
    ///
    /// 只有交互式 shell 会话会发它。宿主可以放心地把同一份参数交给别的用途
    /// (如 [`SshFs`] —— 那时主机密钥已经落地,不会卡在「没人确认」上)。
    Connected,
    /// 会话结束。`exit_status` 是远端 shell 的退出码(拿不到就是 `None`);
    /// `reason` 只在**异常**结束时才有(连接失败 / 传输中断),正常退出为 `None`。
    Closed {
        exit_status: Option<u32>,
        reason: Option<String>,
    },
}

/// 从会话线程发事件的出口(包装一下,省得每个调用点都写 `let _ = ...` )。
///
/// `Clone`:主机密钥确认也走这个口子,而握手回调拿不到线程里的那个实例。
#[derive(Clone)]
pub(crate) struct EventSink(UnboundedSender<SshEvent>);

impl EventSink {
    pub(crate) fn new(sender: UnboundedSender<SshEvent>) -> Self {
        Self(sender)
    }

    pub(crate) fn status(&self, text: impl Into<String>) {
        let _ = self.0.unbounded_send(SshEvent::Status(text.into()));
    }

    /// 把主机密钥确认请求交给宿主(见 [`SshEvent::HostKeyPrompt`])。
    pub(crate) fn host_key_prompt(&self, prompt: HostKeyPrompt) {
        let _ = self.0.unbounded_send(SshEvent::HostKeyPrompt(prompt));
    }

    /// 会话已经可以用了(见 [`SshEvent::Connected`])。
    fn connected(&self) {
        let _ = self.0.unbounded_send(SshEvent::Connected);
    }

    fn data(&self, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            let _ = self.0.unbounded_send(SshEvent::Data(bytes));
        }
    }

    fn closed(&self, exit_status: Option<u32>, reason: Option<String>) {
        let _ = self.0.unbounded_send(SshEvent::Closed {
            exit_status,
            reason,
        });
    }
}

/// 宿主(终端)发给会话线程的命令。
enum Command {
    /// 把键盘字节送给远端。
    Write(Vec<u8>),
    /// 窗口尺寸变了。
    Resize(SshSize),
    /// 主动断开(关闭标签页 / 丢弃会话)。
    Disconnect,
}

/// 一个已经(或正在)建立的 SSH 会话。
///
/// 它只是个**命令发送端**:真正的连接活在后台线程里,所以
/// [`write`](Self::write) / [`resize`](Self::resize) 都是非阻塞的 —— 终端每敲一个键
/// 都会调,不能卡住渲染线程。
#[derive(Clone)]
pub struct SshSession {
    commands: TokioSender<Command>,
    fs: SshFs,
}

/// 这次连接要远端干什么。
enum Launch {
    /// 交互式 shell:申请 PTY 再申请 shell(终端走这条)。
    Shell(SshSize),
    /// 执行一条命令:**不申请 PTY**,命令跑完会话就结束(与 `ssh host cmd` 同)。
    Exec(String),
}

impl SshSession {
    /// 建一个**交互式 shell** 会话:申请 PTY + shell,尺寸由调用方上报与更新。
    ///
    /// 本函数**不等待连接完成**(TCP + 握手 + 认证可能要好几秒),它只负责把线程拉起来。
    /// 失败一律通过 [`SshEvent`] 上报(`Status` 说明原因,随后 `Closed`),这样调用方只有
    /// 一条事件路径要处理。
    pub fn connect(
        params: SshParams,
        size: SshSize,
        events: UnboundedSender<SshEvent>,
    ) -> std::io::Result<Self> {
        Self::spawn(params, Launch::Shell(size), events)
    }

    /// 建一个**执行单条命令**的会话(与 `ssh host <cmd>` 同语义):没有 PTY、不走 shell 的
    /// 交互模式,命令跑完就带退出码结束。
    ///
    /// 用途是脚本与冒烟测试 —— 「交互式 shell 不会自己退出,所以拿它跑脚本会挂住」这件事
    /// 只能靠真 exec 通道解决。
    pub fn connect_exec(
        params: SshParams,
        command: impl Into<String>,
        events: UnboundedSender<SshEvent>,
    ) -> std::io::Result<Self> {
        Self::spawn(params, Launch::Exec(command.into()), events)
    }

    fn spawn(
        params: SshParams,
        launch: Launch,
        events: UnboundedSender<SshEvent>,
    ) -> std::io::Result<Self> {
        let (commands, receiver) = unbounded_channel();
        let fs = SshFs::new(params.clone());
        let endpoint = params.endpoint();
        std::thread::Builder::new()
            .name(format!("ssh {endpoint}"))
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let sink = EventSink::new(events);
                        sink.status(format!("无法启动 SSH 运行时:{error}"));
                        sink.closed(None, Some(error.to_string()));
                        return;
                    }
                };
                runtime.block_on(run(params, launch, EventSink::new(events), receiver));
            })?;

        Ok(Self { commands, fs })
    }

    /// 把字节送给远端(键盘输入 / 粘贴)。空输入直接丢掉 —— 有些服务端会因此 hang。
    pub fn write(&self, bytes: &[u8]) {
        if !bytes.is_empty() {
            let _ = self.commands.send(Command::Write(bytes.to_vec()));
        }
    }

    /// 上报新的窗口尺寸。只在交互式 shell 会话上有意义(exec 没有 PTY)。
    pub fn resize(&self, size: SshSize) {
        let _ = self.commands.send(Command::Resize(size));
    }

    /// 主动断开。幂等,可以在 `Drop` 之后重复调用。
    pub fn disconnect(&self) {
        let _ = self.commands.send(Command::Disconnect);
    }

    /// 这台主机上的**远端文件系统**(SFTP),给「文件管理器」浏览远端目录用。
    ///
    /// 返回的是同一个句柄([`SshFs`] 是 `Arc` 包着的),可以反复取、也可以长期存着;
    /// 真正的连接要到第一次列目录时才建(见 [`crate::fs`] 模块文档)。
    pub fn fs(&self) -> SshFs {
        self.fs.clone()
    }
}

impl Drop for SshSession {
    /// 会话没了就顺手断开:否则远端会一直挂着一个「用户还在」的 shell(还占着一条 TCP)。
    fn drop(&mut self) {
        self.disconnect();
    }
}

/// 会话线程的入口:无论成功失败,最后都发一条 [`SshEvent::Closed`]。
async fn run(
    params: SshParams,
    launch: Launch,
    sink: EventSink,
    mut commands: UnboundedReceiver<Command>,
) {
    match run_session(&params, &launch, &sink, &mut commands).await {
        Ok(exit_status) => sink.closed(exit_status, None),
        Err(error) => {
            // 失败原因既写进事件流(界面显示),也留一份日志。
            log::warn!("SSH 会话结束:{error}");
            sink.closed(None, Some(error.to_string()));
        }
    }
}

/// 建连 → 认证 → 开通道(PTY+shell 或 exec) → 双向搬字节;返回远端 shell 的退出码。
async fn run_session(
    params: &SshParams,
    launch: &Launch,
    sink: &EventSink,
    commands: &mut UnboundedReceiver<Command>,
) -> Result<Option<u32>, SshError> {
    sink.status(format!("正在连接 {} …", params.endpoint()));
    let socket = tokio::net::TcpStream::connect((params.host.as_str(), params.port))
        .await
        .map_err(|source| SshError::Connect {
            host: params.host.clone(),
            port: params.port,
            source,
        })?;

    let config = Arc::new(client::Config {
        // 交互式终端:关掉 Nagle,否则每次敲键要等 40ms 的延迟确认,手感发黏。
        nodelay: true,
        // 空闲的 SSH 连接会被中间设备(路由器 / 防火墙)静默掐断,而且不会通知两端。
        // 30s 一次的保活既是心跳也是探活。
        keepalive_interval: Some(Duration::from_secs(30)),
        ..Default::default()
    });
    let handler = ClientHandler::new(
        params.user.clone(),
        params.host.clone(),
        params.port,
        params.host_key,
        params.known_hosts.clone(),
        // 终端会话这条路有人可问(界面弹窗);没人的情况见 `HostKeyPolicy::Ask`。
        Some(sink.clone()),
    );
    let mut handle = client::connect_stream(config, socket, handler).await?;

    auth::authenticate(&mut handle, params, sink).await?;

    let channel = handle
        .channel_open_session()
        .await
        .map_err(|error| SshError::ChannelOpen(error.to_string()))?;
    let interactive = match launch {
        Launch::Shell(size) => {
            let size = size.sanitized();
            // 终端模式(termios)留空:让远端用自己的默认值。上报一堆猜出来的 termios 反而
            // 容易把远端 shell 弄成坏状态(比如删字符键不对),窗口尺寸才是我们真正知道的信息。
            channel
                .request_pty(
                    false,
                    &params.term,
                    size.cols,
                    size.rows,
                    size.pixel_width,
                    size.pixel_height,
                    &[],
                )
                .await?;
            channel.request_shell(false).await?;
            true
        }
        Launch::Exec(command) => {
            log::debug!("在 {} 上执行:{command}", params.endpoint());
            channel.exec(false, command.clone()).await?;
            false
        }
    };

    // ⚠️ 拆成读写两半,`select!` 里两支才不会同时借 `channel`(见模块文档)。
    let (mut reader, writer) = channel.split();
    let mut exit_status = None;

    // 握手 / 认证 / 开通道都成功了:告诉宿主「可以用了」。
    // 宿主据此才能安全地复用这份参数去做别的事(如 SFTP 浏览 —— 那时主机密钥已经记下,
    // 否则那条连接没有确认通道、只会失败)。
    if interactive {
        sink.connected();
    }

    loop {
        tokio::select! {
            message = reader.wait() => match message {
                Some(ChannelMsg::Data { data }) => sink.data(data.to_vec()),
                // ext = 1 是 stderr。终端里两者是同一个屏幕,一起画。
                Some(ChannelMsg::ExtendedData { data, .. }) => sink.data(data.to_vec()),
                Some(ChannelMsg::ExitStatus { exit_status: code }) => exit_status = Some(code),
                // EOF / Close / 流断掉都算会话结束。
                Some(ChannelMsg::Eof | ChannelMsg::Close) | None => break,
                Some(_) => {}
            },
            command = commands.recv() => match command {
                Some(Command::Write(bytes)) => {
                    if let Err(error) = writer.data_bytes(bytes).await {
                        return Err(error.into());
                    }
                }
                Some(Command::Resize(size)) => {
                    // exec 会话没有 PTY,尺寸变更无处可报。
                    if !interactive {
                        continue;
                    }
                    let size = size.sanitized();
                    // 尺寸变更失败不该掐断会话(远端可能已经登出),记一笔就好。
                    if let Err(error) = writer
                        .window_change(size.cols, size.rows, size.pixel_width, size.pixel_height)
                        .await
                    {
                        log::debug!("上报窗口尺寸失败:{error}");
                    }
                }
                // 宿主主动断开,或宿主把 `SshSession` 丢了(通道关闭)。
                Some(Command::Disconnect) | None => {
                    let _ = writer.eof().await;
                    break;
                }
            },
        }
    }

    // 礼貌地告别:服务端据此清理 shell 与 PTY,不等 TCP 超时。
    let _ = handle
        .disconnect(Disconnect::ByApplication, "", "en")
        .await;
    Ok(exit_status)
}
