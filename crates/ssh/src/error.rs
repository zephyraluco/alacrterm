//! 本 crate 的错误类型。
//!
//! russh 的 [`russh::client::Handler::Error`] 必须实现了 `From<russh::Error>`,所以握手 /
//! 传输层的失败经 [`SshError::Russh`] 原样透出;其余变体是我们自己加的上下文
//! (连不上哪个地址、密码不对、为什么拒绝主机密钥)。

/// SSH 连接 / 会话相关的失败。
#[derive(Debug, thiserror::Error)]
pub enum SshError {
    /// 传输层 / 协议层错误(russh 原样透出)。
    #[error(transparent)]
    Russh(#[from] russh::Error),

    /// 密钥装载 / `known_hosts` 读写错误。
    #[error(transparent)]
    Key(#[from] russh::keys::Error),

    /// TCP 连不上(带上是哪个地址,方便区分 DNS 失败与端口拒绝)。
    #[error("无法连接 {host}:{port}:{source}")]
    Connect {
        host: String,
        port: u16,
        #[source]
        source: std::io::Error,
    },

    /// `known_hosts` 里记录的密钥与服务器给的**不一致**。
    ///
    /// 这是最需要用户注意的错误:要么服务器换了密钥(正常重装),要么有人在中间。
    /// 与 OpenSSH 一样**绝不自动覆盖**,由用户决定(删掉那一行再连)。
    #[error(
        "主机密钥已变更:{host}:{port} 给的密钥与 known_hosts 第 {line} 行记录的不一致。\
         确认服务器确实换了密钥后,删掉那一行(或那个 host 段)再重连。"
    )]
    HostKeyChanged { host: String, port: u16, line: usize },

    /// 主机密钥校验本身失败(读不到 `known_hosts` 等)。
    #[error("主机密钥校验失败:{0}")]
    HostKey(String),

    /// 所有认证方式都没通过(带上试过哪些)。
    #[error("认证失败({tried}):{reason}")]
    Auth { tried: String, reason: String },

    /// 服务端拒绝打开会话通道。
    #[error("服务端拒绝打开会话通道:{0}")]
    ChannelOpen(String),

    /// 远端文件系统(SFTP)不可用,或某次操作失败。
    ///
    /// 这里只留一句话(而不是把 `russh_sftp` 的错误类型透出去):调用方要么直接把这句话
    /// 显示给用户,要么就此放弃 —— 两类消费都不需要结构化信息。
    #[error("远端文件系统:{0}")]
    Sftp(String),
}
