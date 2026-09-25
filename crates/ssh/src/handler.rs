//! russh 的客户端回调:主机密钥校验 + 认证横幅留痕。
//!
//! 这是**安全关键**的一环 —— russh 在密钥交换时已经验过签名,但「这个公钥是不是我信任的
//! 那一台」只能由回调回答(它的默认实现是**一律拒绝**)。
//!
//! 校验分四种结果(见 [`ClientHandler::check_server_key`]):记录过且一致 ⇒ 接受;没记录过 ⇒
//! 按 [`HostKeyPolicy`] 处理(默认 [`HostKeyPolicy::Ask`]:把指纹问给宿主,界面弹窗让用户选);
//! 记录过但**不一致** ⇒ 一律拒绝(防中间人,绝不自动覆盖)。

use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::channel::oneshot;
use russh::client::Handler;
use russh::keys::PublicKeyOrCertificate;
use russh::keys::known_hosts::{
    check_known_hosts, check_known_hosts_path, learn_known_hosts, learn_known_hosts_path,
};

use crate::error::SshError;
use crate::params::HostKeyPolicy;
use crate::session::EventSink;

/// 等用户回答「信任这台主机吗」的时限。
///
/// 这段等待发生在**握手路径上**,整条连接都卡在这里,所以必须有上限:用户走开了也不至于
/// 永远占着一条 TCP 与一个线程。
pub const HOST_KEY_TIMEOUT: Duration = Duration::from_secs(120);

/// 用户对 [`HostKeyPrompt`] 的回答。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostKeyDecision {
    /// 信任并记进 `known_hosts`(等价于 OpenSSH 问句里的 `yes`)。
    Accept,
    /// 不信任,放弃连接。
    Reject,
}

/// 未知主机密钥的确认请求 —— 宿主(界面)拿它弹窗,用户选择后调 [`Self::respond`]。
///
/// 只有 [`HostKeyPolicy::Ask`] 且这台主机**没在 `known_hosts` 里**时才会出现;密钥变更一律
/// 直接拒绝、不走这条路(那种情况要用户自己清理 `known_hosts`)。
#[derive(Clone)]
pub struct HostKeyPrompt {
    /// 登录用户名(让用户确认「连的是不是我要的那台 / 那个账号」)。
    pub user: String,
    /// 主机名或 IP。
    pub host: String,
    /// 端口。
    pub port: u16,
    /// 密钥算法(如 `ssh-ed25519`):核对指纹时要说清是哪把。
    pub key_type: String,
    /// 指纹(OpenSSH 格式,如 `SHA256:AbC…`),让用户与服务器管理员核对。
    pub fingerprint: String,
    /// 回话口:握手线程在里面等,只认第一次回答。
    responses: Arc<ResponseSlot>,
}

impl HostKeyPrompt {
    /// 造一个请求,并把等待端交出来(握手线程 await 它)。
    pub(crate) fn new(
        user: String,
        host: String,
        port: u16,
        key_type: String,
        fingerprint: String,
    ) -> (Self, oneshot::Receiver<HostKeyDecision>) {
        let (sender, receiver) = oneshot::channel();
        let prompt = Self {
            user,
            host,
            port,
            key_type,
            fingerprint,
            responses: Arc::new(ResponseSlot {
                sender: Mutex::new(Some(sender)),
            }),
        };
        (prompt, receiver)
    }

    /// 回答这次确认。**只有第一次调用有效**(之后握手线程已经走了)。
    pub fn respond(&self, decision: HostKeyDecision) {
        let sender = self
            .responses
            .sender
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(sender) = sender {
            let _ = sender.send(decision);
        }
    }

    /// 端点描述(`user@host:port`),界面标题用。
    pub fn endpoint(&self) -> String {
        format!("{}@{}:{}", self.user, self.host, self.port)
    }
}

/// 手写 [`fmt::Debug`]:回话口没有有用的信息,打印它只是噪音。
impl fmt::Debug for HostKeyPrompt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostKeyPrompt")
            .field("user", &self.user)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("key_type", &self.key_type)
            .field("fingerprint", &self.fingerprint)
            .finish()
    }
}

/// 同一个请求在事件流里会被克隆(回话口是共享的)⇒ 「是不是同一个请求」按地址判断。
impl PartialEq for HostKeyPrompt {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.responses, &other.responses)
    }
}

impl Eq for HostKeyPrompt {}

/// 回话口:第一次回答后置空。
struct ResponseSlot {
    sender: Mutex<Option<oneshot::Sender<HostKeyDecision>>>,
}

/// 客户端回调:主机密钥校验需要的全部上下文。
pub(crate) struct ClientHandler {
    user: String,
    host: String,
    port: u16,
    policy: HostKeyPolicy,
    /// `None` = 用默认的 `~/.ssh/known_hosts`。
    known_hosts: Option<PathBuf>,
    /// 把「信任这台主机吗」问出去的口子;`None` = 没人可问(见 [`HostKeyPolicy::Ask`])。
    events: Option<EventSink>,
}

impl ClientHandler {
    pub(crate) fn new(
        user: String,
        host: String,
        port: u16,
        policy: HostKeyPolicy,
        known_hosts: Option<PathBuf>,
        events: Option<EventSink>,
    ) -> Self {
        Self {
            user,
            host,
            port,
            policy,
            known_hosts,
            events,
        }
    }

    /// 查已知主机(带不带自定义路径分两条 russh 函数)。
    fn check(&self, key: &russh::keys::ssh_key::PublicKey) -> Result<bool, russh::keys::Error> {
        match &self.known_hosts {
            Some(path) => check_known_hosts_path(&self.host, self.port, key, path),
            None => check_known_hosts(&self.host, self.port, key),
        }
    }

    /// 记下新主机(TOFU)。
    fn learn(&self, key: &russh::keys::ssh_key::PublicKey) -> Result<(), russh::keys::Error> {
        match &self.known_hosts {
            Some(path) => learn_known_hosts_path(&self.host, self.port, key, path),
            None => learn_known_hosts(&self.host, self.port, key),
        }
    }

    /// 接受这个主机密钥并按 TOFU 记进 `known_hosts`;`reason` 只用于日志。
    fn trust(&self, key: &russh::keys::ssh_key::PublicKey, reason: &str) -> Result<bool, SshError> {
        match self.learn(key) {
            Ok(()) => {
                log::info!(
                    "已把 {}:{} 的主机密钥记进 known_hosts({reason},指纹 {})",
                    self.host,
                    self.port,
                    key.fingerprint(Default::default())
                );
                Ok(true)
            }
            // 记不下来不影响本次连接(用户可能没有 home 目录的写权限),
            // 但下次还得再信任一遍 —— 值得提醒。
            Err(error) => {
                log::warn!("写入 known_hosts 失败(本次仍继续):{error}");
                Ok(true)
            }
        }
    }

    /// 把指纹交给用户确认,等回答(最多 [`HOST_KEY_TIMEOUT`])。
    async fn ask(
        &self,
        key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<HostKeyDecision, SshError> {
        let Some(events) = &self.events else {
            return Err(SshError::HostKey(format!(
                "{}:{} 的主机密钥不在 known_hosts 里,而这条连接没有可用的确认通道。\
                 请先打开这台主机的终端会话,在那里信任它的主机密钥。",
                self.host, self.port
            )));
        };

        let (prompt, receiver) = HostKeyPrompt::new(
            self.user.clone(),
            self.host.clone(),
            self.port,
            key.algorithm().as_str().to_string(),
            key.fingerprint(Default::default()).to_string(),
        );
        events.host_key_prompt(prompt);

        match tokio::time::timeout(HOST_KEY_TIMEOUT, receiver).await {
            Ok(Ok(decision)) => Ok(decision),
            // 界面那边把请求丢了(窗口关了 / 会话被丢弃)。
            Ok(Err(_)) => Err(SshError::HostKey(format!(
                "{}:{} 的主机密钥确认被中断",
                self.host, self.port
            ))),
            Err(_) => Err(SshError::HostKey(format!(
                "等待 {}:{} 的主机密钥确认超过 {} 秒,已放弃连接",
                self.host,
                self.port,
                HOST_KEY_TIMEOUT.as_secs()
            ))),
        }
    }
}

impl Handler for ClientHandler {
    type Error = SshError;

    /// 校验服务器的主机公钥 —— **唯一的中间人防护**,必须老老实实查 `known_hosts`。
    ///
    /// 结果:
    /// - 记录过且一致 ⇒ 接受;
    /// - 没记录过 + [`HostKeyPolicy::Ask`] ⇒ 问用户(同意才记进 `known_hosts`);
    /// - 没记录过 + [`HostKeyPolicy::AcceptNew`] ⇒ 记进 `known_hosts` 后接受;
    /// - 没记录过 + [`HostKeyPolicy::Strict`] ⇒ 拒绝;
    /// - 记录过但**不一致** ⇒ 一律**报错拒绝**(绝不自动覆盖,见 [`SshError::HostKeyChanged`])。
    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let key = server_public_key.public_key();

        match self.check(&key) {
            Ok(true) => Ok(true),
            Ok(false) => match self.policy {
                // ⚠️ 这里刻意返回 `Err` 而不是 `Ok(false)`:两者都会中止连接,但 russh 对
                // `Ok(false)` 只会报一句 `Unknown server key`,用户看了不知道该怎么办;
                // 自己包一层才能说清「是哪个主机、为什么、怎么解决」。
                HostKeyPolicy::Strict => {
                    log::warn!(
                        "{}:{} 的主机密钥不在 known_hosts 里,策略为 strict ⇒ 拒绝",
                        self.host,
                        self.port
                    );
                    Err(SshError::HostKey(format!(
                        "{}:{} 的主机密钥不在 known_hosts 里(当前策略是 strict:不接受未知主机)。\
                         首次连接请用 accept-new(默认),或先手工把主机密钥加进 known_hosts。",
                        self.host, self.port
                    )))
                }
                HostKeyPolicy::AcceptNew => self.trust(&key, "策略为 accept-new"),
                HostKeyPolicy::Ask => match self.ask(&key).await? {
                    HostKeyDecision::Accept => self.trust(&key, "用户已确认"),
                    HostKeyDecision::Reject => {
                        log::info!("{}:{} 的主机密钥被用户拒绝", self.host, self.port);
                        Err(SshError::HostKey(format!(
                            "{}:{} 的主机密钥未被信任(你选择了拒绝)",
                            self.host, self.port
                        )))
                    }
                },
            },
            Err(russh::keys::Error::KeyChanged { line }) => Err(SshError::HostKeyChanged {
                host: self.host.clone(),
                port: self.port,
                line,
            }),
            Err(error) => Err(SshError::HostKey(error.to_string())),
        }
    }

    /// 服务端的认证横幅(如登录提示、MOTD)。原样透出给用户 —— 它常常包含重要通知。
    async fn auth_banner(
        &mut self,
        banner: &str,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        log::info!("{} 的认证横幅:{banner}", self.host);
        Ok(())
    }
}
