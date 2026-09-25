//! 远端文件系统:走 SFTP 子系统,给「文件管理器」列出远端目录。
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
//! ## 线程模型
//!
//! 与 [`session`](crate::session) 同一套:自带一个 OS 线程 + current-thread 运行时,线程里
//! 长期持有一个 [`SftpSession`];跨边界只用**执行器无关**的 `tokio::mpsc`(请求)与
//! `futures::oneshot`(应答)。宿主不需要引入 tokio,也不需要知道 SFTP 的存在。

use std::sync::{Arc, Mutex};

use futures::channel::oneshot;
use futures::future::BoxFuture;
use russh::client;
use russh_sftp::client::SftpSession;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::StatusCode;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender as TokioSender, unbounded_channel};

use crate::auth;
use crate::error::SshError;
use crate::handler::ClientHandler;
use crate::params::SshParams;
use crate::session::EventSink;

/// 空闲保活间隔(与终端那条连接同一套理由:中间设备会静默掐断空闲连接)。
const KEEPALIVE: std::time::Duration = std::time::Duration::from_secs(30);

/// 远端目录里的一项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    /// 名字(不含路径)。
    pub name: String,
    /// 是不是目录(按服务端上报的类型判断;符号链接算不算目录取决于服务端)。
    pub is_dir: bool,
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
    /// 后台线程的入口;`None` = 还没连过,或上一条已经不可用(下次调用会重连)。
    worker: Mutex<Option<TokioSender<Request>>>,
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
}

enum Reply {
    Home(String),
    Entries(Vec<RemoteEntry>),
    IsDir(bool),
}

/// 起后台线程并返回它的入口(调用方的互斥量保证不会并发起多个)。
fn spawn_worker(params: SshParams) -> TokioSender<Request> {
    let (sender, receiver) = unbounded_channel();
    let endpoint = params.endpoint();
    let spawned = std::thread::Builder::new()
        .name(format!("ssh-fs {endpoint}"))
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
            runtime.block_on(serve(params, receiver));
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
                let _ = request.reply.send(Err(describe(error)));
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
            Some(RemoteEntry {
                name,
                is_dir: entry.file_type().is_dir(),
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
fn describe(error: SftpError) -> SshError {
    let text = match &error {
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
