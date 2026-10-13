//! 远端文件系统:走 SFTP 子系统,给「文件管理器」浏览、下载、上传远端文件,也给预览窗口
//! 「保存」把改过的文本写回远端(见 [`SshFs::upload_one`])。
//!
//! ## 为什么另开一条连接
//!
//! 不复用终端那条 SSH 连接。russh 开新通道要 `&mut Handle`,而终端那条连接一直在自己的
//! 线程里 `select!` 搬字节 —— 把 SFTP 塞进去,要么让每次列目录都卡住键盘输入(远端慢时
//! 能卡好几秒),要么给那条线程加一层互斥与任务调度。
//!
//! 单独一条把两件事彻底解耦,代价是**多一次认证**(密码会在加密信道里再送一遍)。为了不
//! 让这个代价落到「根本没打开文件管理器」的会话上,连接是**懒的**:第一次真正要列目录时
//! 才建,失败也只影响文件管理器,碰不到终端。
//!
//! ## 传输走**第二条**连接(与浏览分开)
//!
//! 上传 / 下载可能要跑几分钟,而浏览(列目录 / 看元数据)必须一直是即时的。两者塞进同一个
//! worker 就意味着「传一个 2 GiB 的文件时展开任何目录都得等它传完」。所以:
//!
//! - **浏览 worker**(`worker`):家目录 / 列目录 / 元数据,一条一条快速伺候。
//! - **传输 worker**(`transfer`):上传 / 下载,自己一条连接、一次跑一个传输(天然排队,
//!   不至于把链路打满)。cancel 是**共享的原子标志**(见 [`CancelToken`])而不是消息 ⇒
//!   取消正在跑的传输不需要那条被占住的通道。
//!
//! 代价是每条会话最多**两次**认证,而且两次都是懒的:只用文件管理器不传文件,就只有一条。
//!
//! ## 线程模型
//!
//! 与 [`session`](crate::session) 同一套:自带一个 OS 线程 + current-thread 运行时,线程里
//! 长期持有一个 [`SftpSession`];跨边界只用**执行器无关**的通道 —— 浏览用 `tokio::mpsc`
//! (请求)与 `futures::oneshot`(应答),传输用 `futures::mpsc`(请求 + 进度/结果)。
//! 宿主不需要引入 tokio,也不需要知道 SFTP 的存在。

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use futures::channel::mpsc as futures_mpsc;
use futures::channel::oneshot;
use futures::future::BoxFuture;
use russh::client;
use russh_sftp::client::SftpSession;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::StatusCode;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender as TokioSender, unbounded_channel};

use crate::auth;
use crate::error::SshError;
use crate::handler::ClientHandler;
use crate::params::SshParams;
use crate::session::EventSink;

/// 空闲保活间隔(与终端那条连接同一套理由:中间设备会静默掐断空闲连接)。
const KEEPALIVE: std::time::Duration = std::time::Duration::from_secs(30);

/// 传输时的读 / 写分块大小。SFTP 的 `READ` / `WRITE` 本来就是按请求走的,64 KiB 是常见的
/// 折中(再大也只省一点点往返,再小则把往返次数翻上去)。
const CHUNK: usize = 64 * 1024;

/// 进度上报的**节流**步长:字节数往前走够这么多才再喊一次。
///
/// 不分块喊是因为一个 1 GiB 的文件有 16384 个分块,而界面按 60 Hz 重绘 —— 上报次数比屏幕
/// 刷新还密就没有意义了。
const PROGRESS_STEP: u64 = 256 * 1024;

/// 远端目录里的一项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    /// 名字(不含路径)。
    pub name: String,
    /// 是不是目录(按服务端上报的类型判断;符号链接算不算目录取决于服务端)。
    pub is_dir: bool,
    /// 大小(字节);目录按服务端上报的值(通常是 4096 之类,不用当真)。
    pub size: u64,
}

/// 远端某个路径的元数据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteMeta {
    pub is_dir: bool,
    pub size: u64,
}

/// 传输的取消令牌。
///
/// 做成**共享的原子标志**而不是一条「取消」消息:传输 worker 一次只跑一个传输,跑起来就
/// 占住了自己那条通道 —— 用消息取消的话,恰恰在「最需要取消」的时候送不进去。
#[derive(Clone, Default, Debug)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// 请求取消。worker 在下一个分块边界放弃(不会立刻中断一次进行中的往返)。
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// 一次传输的进度快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferProgress {
    /// 正在传的条目名(最后一个路径分量)。
    pub label: String,
    /// 已经传完的**文件**数(目录不算,和 [`Self::files_total`] 同口径)。
    pub files_done: u64,
    /// 这次传输一共要传多少个文件(目录会先整个走一遍再开传,所以这个数一开始就是准的)。
    pub files_total: u64,
    /// 已经搬过去的字节数(含正在传的这个文件里已经过去的部分)。
    pub bytes_done: u64,
    /// 这次传输的总字节数(0 = 未知)。
    pub bytes_total: u64,
}

/// 传输过程里的两件事:进度,或者结束(只发一次,发完通道就关)。
#[derive(Debug)]
pub enum TransferEvent {
    Progress(TransferProgress),
    /// 结果:`Ok(())` = 全传完了;`Err` = 失败 / 被取消。
    Done(Result<(), SshError>),
}

/// 一次在跑的传输(上传 / 下载)。
///
/// 宿主拿着 [`Transfer::events`] 一直读,直到读到 [`TransferEvent::Done`]。**把 `events`
/// 丢掉就等于取消**(worker 发现没人要结果就停) —— 窗口关掉时不用专门清理。
#[derive(Debug)]
pub struct Transfer {
    pub events: futures_mpsc::UnboundedReceiver<TransferEvent>,
    pub cancel: CancelToken,
}

impl Transfer {
    /// 请求取消(见 [`CancelToken::cancel`])。
    pub fn cancel(&self) -> &CancelToken {
        &self.cancel
    }
}

/// 传输要做的活。
enum TransferKind {
    /// 把本地这些路径(文件或目录,目录递归)传到远端这个目录里。
    Upload {
        locals: Vec<PathBuf>,
        remote_dir: String,
    },
    /// 把远端这些路径(文件或目录,目录递归)下到本地这个目录里,**保持文件名**。
    Download {
        remotes: Vec<String>,
        local_dir: PathBuf,
    },
    /// 把远端这一个文件下到本地的**指定路径**(文件名由调用方定,预览缓存用)。
    DownloadOne { remote: String, local_file: PathBuf },
    /// 把本地这一个文件写到远端的**指定路径**(远端路径由调用方给全,预览窗口「保存」用)。
    UploadOne {
        local_file: PathBuf,
        remote_file: String,
    },
}

/// 一台 SSH 主机上的远端文件系统。
///
/// 只是个**句柄**:真正的连接活在后台线程里(见模块文档),所以 `Clone` 只是复制句柄,
/// [`PartialEq`] 比的也是「是不是同一个会话的文件系统」—— 宿主据此判断「当前会话变了没」。
#[derive(Clone)]
pub struct SshFs(Arc<Inner>);

struct Inner {
    /// 建连要用的参数(与终端会话同一份)。
    params: SshParams,
    /// **浏览** worker 的入口;`None` = 还没连过,或上一条已经不可用(下次调用会重连)。
    worker: Mutex<Option<TokioSender<Request>>>,
    /// **传输** worker 的入口(另一条连接,见模块文档);同样是懒的。
    transfer: Mutex<Option<TokioSender<TransferRequest>>>,
}

impl PartialEq for SshFs {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl SshFs {
    /// 建一个句柄(**不连网**,第一次请求时才连)。
    ///
    /// 通常不直接调它:终端那边用 [`SshSession::fs`](crate::SshSession::fs) 拿到与那条会话
    /// 同参数的句柄。单独构造的场合是「只要远端文件、不开终端」。
    pub fn new(params: SshParams) -> Self {
        Self(Arc::new(Inner {
            params,
            worker: Mutex::new(None),
            transfer: Mutex::new(None),
        }))
    }

    /// 远端家目录(绝对路径),用作文件管理器的起始根目录。
    ///
    /// 远端 shell 的 `cd` 拿不到(那要改远端 prompt,做法见 `terminal::platform` 里本地
    /// PowerShell 那套),所以远端根目录**不跟随终端**:从家目录起步,由用户自己跳。
    pub fn home_dir(&self) -> BoxFuture<'static, Result<String, SshError>> {
        expect(self.send(RequestKind::Home), |reply| match reply {
            Reply::Home(path) => Ok(path),
            _ => Err(unexpected()),
        })
    }

    /// 列一层目录(不递归)。`path` 是 POSIX 绝对路径。
    pub fn list_dir(
        &self,
        path: impl Into<String>,
    ) -> BoxFuture<'static, Result<Vec<RemoteEntry>, SshError>> {
        expect(self.send(RequestKind::List(path.into())), |reply| {
            match reply {
                Reply::Entries(entries) => Ok(entries),
                _ => Err(unexpected()),
            }
        })
    }

    /// 这个路径是不是目录(输入框跳转前用它把关)。
    pub fn is_dir(&self, path: impl Into<String>) -> BoxFuture<'static, Result<bool, SshError>> {
        expect(self.send(RequestKind::IsDir(path.into())), |reply| {
            match reply {
                Reply::IsDir(is_dir) => Ok(is_dir),
                _ => Err(unexpected()),
            }
        })
    }

    /// 某个路径的元数据(预览窗口显示大小 / 判断文件还是目录)。
    pub fn stat(&self, path: impl Into<String>) -> BoxFuture<'static, Result<RemoteMeta, SshError>> {
        expect(self.send(RequestKind::Stat(path.into())), |reply| {
            match reply {
                Reply::Meta(meta) => Ok(meta),
                _ => Err(unexpected()),
            }
        })
    }

    /// 把本地这些路径传到远端目录里(目录递归;远端已有的目录直接用)。
    pub fn upload(&self, locals: Vec<PathBuf>, remote_dir: impl Into<String>) -> Transfer {
        self.send_transfer(TransferKind::Upload {
            locals,
            remote_dir: remote_dir.into(),
        })
    }

    /// 把远端这些路径下到本地目录里,保持文件名(目录递归)。
    pub fn download(&self, remotes: Vec<String>, local_dir: impl Into<PathBuf>) -> Transfer {
        self.send_transfer(TransferKind::Download {
            remotes,
            local_dir: local_dir.into(),
        })
    }

    /// 把远端这一个文件下到本地**指定的路径**(文件名由调用方定)。
    pub fn download_one(&self, remote: impl Into<String>, local_file: impl Into<PathBuf>) -> Transfer {
        self.send_transfer(TransferKind::DownloadOne {
            remote: remote.into(),
            local_file: local_file.into(),
        })
    }

    /// 把本地这一个文件写到远端**指定的路径**(同名远端文件被**覆盖 / 截断**)。
    ///
    /// 与 [`Self::upload`] 的区别是**不改名、不拼目录**:远端路径由调用方给全。这是
    /// [`Self::download_one`] 的反向,「保存」远端文件要写的正是它打开时那个路径。
    pub fn upload_one(
        &self,
        local_file: impl Into<PathBuf>,
        remote_file: impl Into<String>,
    ) -> Transfer {
        self.send_transfer(TransferKind::UploadOne {
            local_file: local_file.into(),
            remote_file: remote_file.into(),
        })
    }

    /// 把请求交给后台线程,拿到一条应答通道;线程还没起(或已失效)就顺手起一个。
    ///
    /// 发送失败意味着线程刚退出:清掉句柄**重试一次**,让「上一次连接断了」不至于让用户
    /// 再点一次才成功。
    fn send(&self, kind: RequestKind) -> oneshot::Receiver<Result<Reply, SshError>> {
        let (reply, receiver) = oneshot::channel();
        let mut request = Request { kind, reply };

        let mut slot = self.0.worker.lock().expect("远端文件系统句柄互斥量中毒");
        for _ in 0..2 {
            let sender = match slot.as_ref() {
                Some(sender) if !sender.is_closed() => sender.clone(),
                _ => {
                    let sender = spawn_worker(self.0.params.clone());
                    *slot = Some(sender.clone());
                    sender
                }
            };
            match sender.send(request) {
                Ok(()) => return receiver,
                // 线程已退出:请求原样还了回来,重起一个线程再发。
                Err(returned) => {
                    request = returned.0;
                    *slot = None;
                }
            }
        }
        // 连起两次都没成:应答通道被丢掉 ⇒ 调用方收到「线程已退出」。
        receiver
    }

    /// 把传输交给**传输 worker**(另一条连接,见模块文档);线程还没起(或已失效)就顺手起一个。
    ///
    /// 与 [`Self::send`] 同一套「发失败就重起一次」的处理。区别是这里**没有应答通道**:
    /// 结果走 [`TransferEvent::Done`],所以线程没起成也不会有回执 —— 而是 `events` 通道
    /// 直接读到 `None`(调用方按「线程已退出」处理)。
    fn send_transfer(&self, kind: TransferKind) -> Transfer {
        let (events, receiver) = futures_mpsc::unbounded();
        let cancel = CancelToken::default();
        let mut request = TransferRequest {
            kind,
            events,
            cancel: cancel.clone(),
        };

        let mut slot = self
            .0
            .transfer
            .lock()
            .expect("远端文件传输句柄互斥量中毒");
        for _ in 0..2 {
            let sender = match slot.as_ref() {
                Some(sender) if !sender.is_closed() => sender.clone(),
                _ => {
                    let sender = spawn_transfer_worker(self.0.params.clone());
                    *slot = Some(sender.clone());
                    sender
                }
            };
            match sender.send(request) {
                Ok(()) => return Transfer { events: receiver, cancel },
                // 线程已退出:请求原样还了回来,重起一个线程再发。
                Err(returned) => {
                    request = returned.0;
                    *slot = None;
                }
            }
        }
        Transfer { events: receiver, cancel }
    }
}

/// 请求与应答一一对应,正常到不了这里。
fn unexpected() -> SshError {
    SshError::Sftp("远端文件系统的应答与请求对不上".to_string())
}

/// 把应答通道包成 [`BoxFuture`],并套一层「线程没了」的兜底。
fn expect<T: 'static>(
    receiver: oneshot::Receiver<Result<Reply, SshError>>,
    extract: fn(Reply) -> Result<T, SshError>,
) -> BoxFuture<'static, Result<T, SshError>> {
    Box::pin(async move {
        match receiver.await {
            Ok(Ok(reply)) => extract(reply),
            Ok(Err(error)) => Err(error),
            // 线程在答复前就退了(建连崩了 / 进程收尾)。
            Err(_) => Err(SshError::Sftp("远端文件系统线程已退出".to_string())),
        }
    })
}

// ---------------------------------------------------------------- 线程侧

/// 一次请求。
struct Request {
    kind: RequestKind,
    /// 结果回这条 one-shot(调用方 await 它)。
    reply: oneshot::Sender<Result<Reply, SshError>>,
}

enum RequestKind {
    /// 家目录。
    Home,
    /// 列一层目录(POSIX 路径)。
    List(String),
    /// 这个路径是不是目录。
    IsDir(String),
    /// 这个路径的元数据。
    Stat(String),
}

enum Reply {
    Home(String),
    Entries(Vec<RemoteEntry>),
    IsDir(bool),
    Meta(RemoteMeta),
}

/// 一次传输请求。没有应答通道:进度与结果都走 `events`(见 [`TransferEvent`])。
struct TransferRequest {
    kind: TransferKind,
    events: futures_mpsc::UnboundedSender<TransferEvent>,
    cancel: CancelToken,
}

/// 起**浏览** worker(线程 + 运行时 + 一条连接),返回它的入口。
fn spawn_worker(params: SshParams) -> TokioSender<Request> {
    let name = format!("ssh-fs {}", params.endpoint());
    spawn_thread(name, move |requests| Box::pin(serve(params, requests)))
}

/// 起**传输** worker(另一条连接,见模块文档),返回它的入口。
fn spawn_transfer_worker(params: SshParams) -> TokioSender<TransferRequest> {
    let name = format!("ssh-fs transfer {}", params.endpoint());
    spawn_thread(name, move |requests| Box::pin(serve_transfers(params, requests)))
}

/// 起一个后台线程:自带 current-thread 运行时,在里面把 `serve` 跑完,返回它的入口。
///
/// 调用方的互斥量保证同一类 worker 不会并发起多个。
fn spawn_thread<T: Send + 'static>(
    name: String,
    serve: impl FnOnce(UnboundedReceiver<T>) -> BoxFuture<'static, ()> + Send + 'static,
) -> TokioSender<T> {
    let (sender, receiver) = unbounded_channel();
    let spawned = std::thread::Builder::new()
        .name(name)
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    log::warn!("无法启动远端文件系统运行时:{error}");
                    return;
                }
            };
            runtime.block_on(serve(receiver));
        });
    if let Err(error) = spawned {
        // 线程起不来 ⇒ 接收端已经没了,后续 `send` 一律失败,调用方会看到「线程已退出」。
        log::warn!("无法启动远端文件系统线程:{error}");
    }
    sender
}

/// 线程主循环:按需建 SFTP 连接,然后一条条伺候请求。
async fn serve(params: SshParams, mut requests: UnboundedReceiver<Request>) {
    let mut sftp: Option<SftpSession> = None;

    while let Some(request) = requests.recv().await {
        if sftp.is_none() {
            match connect(&params).await {
                Ok(session) => sftp = Some(session),
                Err(error) => {
                    let _ = request.reply.send(Err(error));
                    continue;
                }
            }
        }
        let session = sftp.as_ref().expect("上面刚确认过");

        let outcome = match &request.kind {
            RequestKind::Home => session.canonicalize(".").await.map(Reply::Home),
            RequestKind::List(path) => list_dir(session, path).await.map(Reply::Entries),
            RequestKind::IsDir(path) => session
                .metadata(path)
                .await
                .map(|attributes| Reply::IsDir(attributes.is_dir())),
            RequestKind::Stat(path) => session.metadata(path).await.map(|attributes| {
                Reply::Meta(RemoteMeta {
                    is_dir: attributes.is_dir(),
                    size: attributes.len(),
                })
            }),
        };

        match outcome {
            Ok(reply) => {
                let _ = request.reply.send(Ok(reply));
            }
            Err(error) => {
                // 服务端报的**状态错误**(没有这个路径 / 没权限)只是这一条路径的事,连接还能用;
                // 其余(IO / 超时 / 协议)说明这条连接已经不可靠,丢掉让下次请求重连。
                if !matches!(error, SftpError::Status(_)) {
                    sftp = None;
                }
                let _ = request.reply.send(Err(describe(&error)));
            }
        }
    }

    if let Some(session) = sftp {
        let _ = session.close().await;
    }
}

/// 建连 + 认证 + 开 sftp 子系统。认证复用终端那条路的同一套顺序(agent → 私钥 → 密码)。
async fn connect(params: &SshParams) -> Result<SftpSession, SshError> {
    let socket = TcpStream::connect((params.host.as_str(), params.port))
        .await
        .map_err(|source| SshError::Connect {
            host: params.host.clone(),
            port: params.port,
            source,
        })?;

    let config = Arc::new(client::Config {
        nodelay: true,
        keepalive_interval: Some(KEEPALIVE),
        ..Default::default()
    });
    let handler = ClientHandler::new(
        params.user.clone(),
        params.host.clone(),
        params.port,
        params.host_key,
        params.known_hosts.clone(),
        // 这条连接没人在旁边弹窗 ⇒ 未知主机密钥直接失败(见 `HostKeyPolicy::Ask`)。
        // 实际到不了这里:句柄来自已经建好的终端会话,那时主机密钥已经记进 known_hosts。
        None,
    );
    let mut handle = client::connect_stream(config, socket, handler).await?;

    // 认证进度不进终端(那条连接有自己的进度行):丢掉接收端,发送失败会被静默忽略。
    let (events, _) = futures::channel::mpsc::unbounded();
    let sink = EventSink::new(events);
    auth::authenticate(&mut handle, params, &sink).await?;

    let channel = handle
        .channel_open_session()
        .await
        .map_err(|error| SshError::ChannelOpen(error.to_string()))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|error| SshError::Sftp(format!("服务端没有提供 sftp 子系统:{error}")))?;

    SftpSession::new(channel.into_stream())
        .await
        .map_err(|error| SshError::Sftp(error.to_string()))
}

/// 列一层目录,顺序与本地一致:目录在前、名字不区分大小写。
async fn list_dir(session: &SftpSession, path: &str) -> Result<Vec<RemoteEntry>, SftpError> {
    let entries = session.read_dir(path).await?;
    let mut items: Vec<RemoteEntry> = entries
        .filter_map(|entry| {
            let name = entry.file_name();
            // 有些服务端会把 `.` 与 `..` 也列出来,树里不需要它们。
            if name == "." || name == ".." {
                return None;
            }
            let is_dir = entry.file_type().is_dir();
            Some(RemoteEntry {
                name,
                is_dir,
                // 目录那点大小(4096 之类)没有意义,直接归零,免得界面把它当文件大小显示。
                size: if is_dir { 0 } else { entry.metadata().len() },
            })
        })
        .collect();
    items.sort_by_cached_key(|entry| (!entry.is_dir, entry.name.to_lowercase()));
    Ok(items)
}

/// 把 SFTP 的失败翻译成给用户看的一句话。
///
/// 常见状态码给中文(用户最可能撞上的就是这几个),其余原样透出 —— 宁可露出一句英文,
/// 也不要凭猜测编一个可能误导人的说法。
fn describe(error: &SftpError) -> SshError {
    let text = match error {
        SftpError::Status(status) => match status.status_code {
            StatusCode::NoSuchFile => "远端没有这个路径".to_string(),
            StatusCode::PermissionDenied => "远端拒绝访问这个路径(权限不足)".to_string(),
            StatusCode::NoConnection | StatusCode::ConnectionLost => {
                "与远端的文件系统连接已断开".to_string()
            }
            _ => error.to_string(),
        },
        SftpError::Timeout => "远端文件系统响应超时".to_string(),
        SftpError::IO(_) | SftpError::UnexpectedPacket | SftpError::UnexpectedBehavior(_) => {
            format!("远端文件系统连接异常:{error}")
        }
        SftpError::Limited(_) => format!("远端文件系统超出限制:{error}"),
    };
    SshError::Sftp(text)
}

// ---------------------------------------------------------------- 传输

/// 传输失败:`describe` 那句话 + **这条连接还能不能用**。
///
/// 与浏览那侧的判据一致(见 `serve`):服务端报的**状态错误**(没有这个路径 / 没权限)只是
/// 这一条路径的事,连接还能用;其余(IO / 超时 / 协议)说明连接已经不可靠,丢掉让下次重连。
struct TransferFailure {
    error: SshError,
    fatal: bool,
}

impl TransferFailure {
    /// 直接来自 [`SftpSession`] 的错误。
    fn from_sftp(error: SftpError) -> Self {
        let fatal = !matches!(error, SftpError::Status(_));
        Self {
            error: describe(&error),
            fatal,
        }
    }

    /// 来自 [`russh_sftp::client::fs::File`] 那套 `io::Error` 的错误。
    ///
    /// `io::Error` 里裹着的多半还是 SFTP 的错误(打开 / 读到的那条路就是 `io::Error::new`
    /// 把原错误当 source 存着)⇒ 裹着的就按 SFTP 那套措辞翻译;没裹着的(写失败那种只存了
    /// 服务端的一句话)原样透出,并按「连接不可靠」处理。
    fn from_io(error: io::Error) -> Self {
        match error
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<SftpError>())
        {
            Some(inner) => Self {
                error: describe(inner),
                fatal: !matches!(inner, SftpError::Status(_)),
            },
            None => Self {
                error: SshError::Sftp(error.to_string()),
                fatal: true,
            },
        }
    }

    /// 本地那头的 IO 失败(打不开源文件 / 写不了目标文件);与远端连接无关。
    fn local(error: io::Error) -> Self {
        Self {
            error: SshError::Sftp(format!("本地文件出错:{error}")),
            fatal: false,
        }
    }

    fn cancelled() -> Self {
        Self {
            error: SshError::Cancelled,
            fatal: false,
        }
    }
}

/// 传输 worker 主循环:按需建**第二条** SFTP 连接,然后一个传输一个传输地跑。
///
/// 一次只跑一个(天然排队,不至于把链路打满);取消靠共享的 [`CancelToken`],不需要这条
/// 被占住的通道(见模块文档)。
async fn serve_transfers(params: SshParams, mut requests: UnboundedReceiver<TransferRequest>) {
    let mut sftp: Option<SftpSession> = None;

    while let Some(request) = requests.recv().await {
        if sftp.is_none() {
            match connect(&params).await {
                Ok(session) => sftp = Some(session),
                Err(error) => {
                    let _ = request.events.unbounded_send(TransferEvent::Done(Err(error)));
                    continue;
                }
            }
        }
        let session = sftp.as_ref().expect("上面刚确认过");

        let outcome = run_transfer(&request.kind, session, &request.events, &request.cancel).await;
        if let Err(failure) = &outcome
            && failure.fatal
        {
            sftp = None;
        }
        let _ = request
            .events
            .unbounded_send(TransferEvent::Done(outcome.map_err(|failure| failure.error)));
    }

    if let Some(session) = sftp {
        let _ = session.close().await;
    }
}

/// 跑一次传输。先把要传的东西摊成清单再开传 ⇒ 进度条一开始就知道总数 / 总字节数。
async fn run_transfer(
    kind: &TransferKind,
    session: &SftpSession,
    events: &futures_mpsc::UnboundedSender<TransferEvent>,
    cancel: &CancelToken,
) -> Result<(), TransferFailure> {
    let uploading = matches!(
        kind,
        TransferKind::Upload { .. } | TransferKind::UploadOne { .. }
    );
    let jobs = match kind {
        TransferKind::Upload {
            locals,
            remote_dir,
        } => collect_uploads(locals, remote_dir)?,
        TransferKind::Download {
            remotes,
            local_dir,
        } => collect_downloads(session, remotes, local_dir).await?,
        TransferKind::DownloadOne { remote, local_file } => {
            let meta = session
                .metadata(remote.as_str())
                .await
                .map_err(TransferFailure::from_sftp)?;
            vec![Job {
                source: remote.clone(),
                target: local_file.clone(),
                size: meta.len(),
                is_dir: false,
            }]
        }
        // 单个文件写回:清单就一项(来源 = 本地那份,目标 = 调用方给全的远端路径)。
        TransferKind::UploadOne {
            local_file,
            remote_file,
        } => {
            let meta = std::fs::metadata(local_file).map_err(TransferFailure::local)?;
            vec![Job {
                source: local_file.to_string_lossy().into_owned(),
                target: PathBuf::from(remote_file),
                size: meta.len(),
                is_dir: false,
            }]
        }
    };

    let mut reporter = Reporter::new(events, cancel, &jobs);
    for job in &jobs {
        reporter.begin(&job.label())?;
        if job.is_dir {
            // 上传:在远端建;下载:在本地建。
            if uploading {
                ensure_remote_dir(session, &job.target).await?;
            } else {
                tokio::fs::create_dir_all(&job.target)
                    .await
                    .map_err(TransferFailure::local)?;
            }
        } else if uploading {
            upload_file(session, Path::new(&job.source), &job.target, &mut reporter).await?;
        } else {
            download_file(session, &job.source, &job.target, &mut reporter).await?;
        }
        reporter.finish(job.is_dir)?;
    }
    Ok(())
}

/// 远端路径拼接(远端一律 POSIX 的 `/`)。
fn join_dir(dir: &str, name: &str) -> String {
    format!("{}/{}", dir.trim_end_matches('/'), name)
}

/// 传输清单里的一项。`source` / `target` 的含义按方向反过来(上传:本地→远端)。
struct Job {
    source: String,
    target: PathBuf,
    /// 字节数;目录为 0。
    size: u64,
    is_dir: bool,
}

impl Job {
    /// 进度行里显示的名字(取源那头的最后一段:上传看本地名,下载看远端名)。
    fn label(&self) -> String {
        self.source
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&self.source)
            .to_string()
    }
}

/// 把本地的这些路径(目录递归)摊成上传清单,`remote_dir` 是远端的目标目录。
///
/// 用栈做深度优先,**入栈顺序保证父目录排在子目录前面**(建目录要先有父目录)。
fn collect_uploads(locals: &[PathBuf], remote_dir: &str) -> Result<Vec<Job>, TransferFailure> {
    let mut jobs = Vec::new();
    let mut stack: Vec<(PathBuf, String)> = Vec::new();
    for local in locals {
        let name = local
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| local.to_string_lossy().into_owned());
        stack.push((local.clone(), join_dir(remote_dir, &name)));
    }
    stack.reverse();
    while let Some((local, remote)) = stack.pop() {
        let link = std::fs::symlink_metadata(&local).map_err(TransferFailure::local)?;
        let is_symlink = link.file_type().is_symlink();
        // 符号链接指向哪里要单独问一次(上面那个问的是链接本身)。
        let meta = if is_symlink {
            std::fs::metadata(&local).unwrap_or(link)
        } else {
            link
        };
        if meta.is_dir() {
            if is_symlink {
                // ⚠️ 指向目录的符号链接**不递归**:指回上层目录的链接会让遍历转不出来。
                log::debug!("跳过指向目录的符号链接(不递归):{}", local.display());
                continue;
            }
            jobs.push(Job {
                source: local.to_string_lossy().into_owned(),
                target: PathBuf::from(&remote),
                size: 0,
                is_dir: true,
            });
            let mut nested: Vec<(PathBuf, String)> = Vec::new();
            for child in std::fs::read_dir(&local).map_err(TransferFailure::local)? {
                let child = child.map_err(TransferFailure::local)?;
                let name = child.file_name().to_string_lossy().into_owned();
                nested.push((child.path(), join_dir(&remote, &name)));
            }
            // 读目录的顺序不保证稳定,排一下让「第 n/m 个」看着是有序的。
            nested.sort_by(|a, b| a.1.cmp(&b.1));
            stack.extend(nested.into_iter().rev());
        } else {
            jobs.push(Job {
                source: local.to_string_lossy().into_owned(),
                target: PathBuf::from(&remote),
                size: meta.len(),
                is_dir: false,
            });
        }
    }
    Ok(jobs)
}

/// 把远端的这些路径(目录递归)摊成下载清单,`local_dir` 是本地的目标目录。
async fn collect_downloads(
    session: &SftpSession,
    remotes: &[String],
    local_dir: &Path,
) -> Result<Vec<Job>, TransferFailure> {
    let mut jobs = Vec::new();
    let mut stack: Vec<(String, PathBuf)> = Vec::new();
    for remote in remotes {
        let name = remote
            .rsplit('/')
            .find(|name| !name.is_empty())
            .unwrap_or(remote.as_str());
        stack.push((remote.clone(), local_dir.join(name)));
    }
    // 与上传同一条理由:让父目录先出栈。
    stack.reverse();
    while let Some((remote, local)) = stack.pop() {
        let meta = session
            .metadata(remote.as_str())
            .await
            .map_err(TransferFailure::from_sftp)?;
        if meta.is_dir() {
            jobs.push(Job {
                source: remote.clone(),
                target: local.clone(),
                size: 0,
                is_dir: true,
            });
            let mut nested: Vec<(String, PathBuf)> = Vec::new();
            for entry in session
                .read_dir(remote.as_str())
                .await
                .map_err(TransferFailure::from_sftp)?
            {
                let name = entry.file_name();
                if name == "." || name == ".." {
                    continue;
                }
                nested.push((join_dir(&remote, &name), local.join(name)));
            }
            nested.sort_by(|a, b| a.0.cmp(&b.0));
            stack.extend(nested.into_iter().rev());
        } else {
            jobs.push(Job {
                source: remote.clone(),
                target: local,
                size: meta.len(),
                is_dir: false,
            });
        }
    }
    Ok(jobs)
}

/// 进度上报。攒够 [`PROGRESS_STEP`] 才发一次,免得每个分块都惊动一次界面。
struct Reporter<'a> {
    events: &'a futures_mpsc::UnboundedSender<TransferEvent>,
    cancel: &'a CancelToken,
    files_total: u64,
    bytes_total: u64,
    files_done: u64,
    bytes_done: u64,
    /// 上次上报时的字节数。
    reported: u64,
    label: String,
}

impl<'a> Reporter<'a> {
    fn new(
        events: &'a futures_mpsc::UnboundedSender<TransferEvent>,
        cancel: &'a CancelToken,
        jobs: &[Job],
    ) -> Self {
        Self {
            events,
            cancel,
            files_total: jobs.iter().filter(|job| !job.is_dir).count() as u64,
            bytes_total: jobs.iter().map(|job| job.size).sum(),
            files_done: 0,
            bytes_done: 0,
            reported: 0,
            label: String::new(),
        }
    }

    /// 取消检查。
    fn check(&self) -> Result<(), TransferFailure> {
        if self.cancel.is_cancelled() {
            return Err(TransferFailure::cancelled());
        }
        Ok(())
    }

    /// 开始传一个条目:无条件报一次(至少让进度行的名字立刻变过来)。
    fn begin(&mut self, label: &str) -> Result<(), TransferFailure> {
        self.check()?;
        self.label = label.to_string();
        self.report()
    }

    /// 又搬过去 `n` 字节;攒够 [`PROGRESS_STEP`] 就报一次。
    fn advance(&mut self, n: u64) -> Result<(), TransferFailure> {
        self.bytes_done += n;
        self.check()?;
        if self.bytes_done - self.reported >= PROGRESS_STEP {
            self.report()?;
        }
        Ok(())
    }

    /// 一个条目走完了。⚠️ **目录不计入「第 n/m 个」** —— `files_total` 只数文件,两边口径
    /// 必须一致,否则「3/2 个」会露到界面上。
    fn finish(&mut self, is_dir: bool) -> Result<(), TransferFailure> {
        if !is_dir {
            self.files_done += 1;
        }
        self.check()?;
        self.report()
    }

    /// 发一次进度。发不出去 = 宿主把 `events` 丢了(窗口关了 / 行被取消了)⇒ 按取消算。
    fn report(&mut self) -> Result<(), TransferFailure> {
        let progress = TransferProgress {
            label: self.label.clone(),
            files_done: self.files_done,
            files_total: self.files_total,
            bytes_done: self.bytes_done,
            bytes_total: self.bytes_total,
        };
        self.reported = self.bytes_done;
        self.events
            .unbounded_send(TransferEvent::Progress(progress))
            .map_err(|_| TransferFailure::cancelled())
    }
}

/// 远端建目录;已经有了就直接用(往已有目录里上传是常态)。
async fn ensure_remote_dir(session: &SftpSession, dir: &Path) -> Result<(), TransferFailure> {
    let dir = remote_path(dir);
    if session
        .try_exists(dir.as_str())
        .await
        .map_err(TransferFailure::from_sftp)?
    {
        return Ok(());
    }
    session
        .create_dir(dir.as_str())
        .await
        .map_err(TransferFailure::from_sftp)
}

/// 本地路径拼出来的远端路径:`PathBuf` 在 Windows 上会用 `\`,远端只认 `/`。
fn remote_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// 本地文件 → 远端路径,分块搬,每块都查一次取消。
async fn upload_file(
    session: &SftpSession,
    local: &Path,
    remote: &Path,
    reporter: &mut Reporter<'_>,
) -> Result<(), TransferFailure> {
    let mut source = tokio::fs::File::open(local)
        .await
        .map_err(TransferFailure::local)?;
    let mut target = session
        .create(remote_path(remote))
        .await
        .map_err(TransferFailure::from_sftp)?;

    let mut buffer = vec![0u8; CHUNK];
    loop {
        let read = source
            .read(&mut buffer)
            .await
            .map_err(TransferFailure::local)?;
        if read == 0 {
            break;
        }
        target
            .write_all(&buffer[..read])
            .await
            .map_err(TransferFailure::from_io)?;
        reporter.advance(read as u64)?;
    }
    // `close` = 把在途的写等回来 + 发 CLOSE;光靠 `Drop` 不等在途写。
    target.close().await.map_err(TransferFailure::from_io)?;
    Ok(())
}

/// 远端路径 → 本地文件,分块搬,每块都查一次取消。
async fn download_file(
    session: &SftpSession,
    remote: &str,
    local: &Path,
    reporter: &mut Reporter<'_>,
) -> Result<(), TransferFailure> {
    let mut source = session
        .open(remote)
        .await
        .map_err(TransferFailure::from_sftp)?;
    // 目标目录在清单阶段可能还没建(单文件下载没有目录项),这里补一次。
    if let Some(parent) = local.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(TransferFailure::local)?;
    }
    let mut target = tokio::fs::File::create(local)
        .await
        .map_err(TransferFailure::local)?;

    let mut buffer = vec![0u8; CHUNK];
    loop {
        let read = source
            .read(&mut buffer)
            .await
            .map_err(TransferFailure::from_io)?;
        if read == 0 {
            break;
        }
        target
            .write_all(&buffer[..read])
            .await
            .map_err(TransferFailure::local)?;
        reporter.advance(read as u64)?;
    }
    target.flush().await.map_err(TransferFailure::local)?;
    Ok(())
}
