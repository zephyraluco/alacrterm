//! 认证:按 OpenSSH 的习惯**依次尝试**,任一成功即停。
//!
//! 顺序 = ssh-agent → 私钥文件 → 密码。为什么是这个顺序:
//! - agent 在前,因为它通常装着用户最好用的那把钥匙(也可能是硬件令牌,私钥根本拿不到);
//! - 私钥文件次之,够用且不需要用户干预;
//! - 密码最后,因为它必须由用户输入,是成本最高的一种。
//!
//! 每一步失败都**继续往下试**(SSH 的认证本身就是「多种方法来回试」的协议:
//! 服务端会在失败回复里告诉客户端还允许哪些方法)。全部失败才报
//! [`SshError::Auth`],并把「试过什么」带上 —— 这是用户排查的唯一线索。

use std::path::PathBuf;
use std::sync::Arc;

use russh::client;
use russh::keys::agent::client::{AgentClient, AgentStream};
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, load_secret_key};

use crate::error::SshError;
use crate::handler::ClientHandler;
use crate::params::{SshAuth, SshParams};
use crate::session::EventSink;

/// agent 客户端擦掉具体传输类型后的形态(Unix 走 unix socket,Windows 走命名管道 / Pageant)。
type Agent = AgentClient<Box<dyn AgentStream + Send + Unpin>>;

/// 默认私钥文件名(按尝试顺序)。与 OpenSSH 一样只认这三种现代算法。
const DEFAULT_KEY_NAMES: [&str; 3] = ["id_ed25519", "id_ecdsa", "id_rsa"];

/// 完成认证;失败时返回带上下文的 [`SshError::Auth`]。
pub(crate) async fn authenticate(
    handle: &mut client::Handle<ClientHandler>,
    params: &SshParams,
    events: &EventSink,
) -> Result<(), SshError> {
    // RSA 需要协商哈希算法(服务端可能只认 rsa-sha2-*);非 RSA 密钥会忽略它。
    let rsa_hash: Option<HashAlg> = handle.best_supported_rsa_hash().await?.flatten();

    let mut tried: Vec<String> = Vec::new();
    let mut reason = String::new();

    if try_agent(handle, params, rsa_hash, events, &mut tried, &mut reason).await? {
        return Ok(());
    }
    if try_key_files(handle, params, rsa_hash, events, &mut tried, &mut reason).await? {
        return Ok(());
    }
    if let SshAuth::Password(password) = &params.auth {
        events.status("正在用密码认证 …");
        tried.push("密码".to_string());
        if handle
            .authenticate_password(&params.user, password)
            .await?
            .success()
        {
            return Ok(());
        }
        reason = "服务端拒绝了密码".to_string();
    }

    if tried.is_empty() {
        reason = "没有可用的认证方式(没找到 ssh-agent、没找到私钥文件、也没提供密码)".to_string();
    }
    Err(SshError::Auth {
        tried: tried.join("、"),
        reason,
    })
}

/// 用 ssh-agent 里的身份认证;成功返回 `true`。
///
/// `Err` 只在**传输层**出问题时返回(连接已坏,继续试别的认证方式没意义)。
async fn try_agent(
    handle: &mut client::Handle<ClientHandler>,
    params: &SshParams,
    rsa_hash: Option<HashAlg>,
    events: &EventSink,
    tried: &mut Vec<String>,
    reason: &mut String,
) -> Result<bool, SshError> {
    let Some(mut agent) = connect_agent().await else {
        log::debug!("没有可用的 ssh-agent");
        return Ok(false);
    };
    let identities = match agent.request_identities().await {
        Ok(identities) => identities,
        Err(error) => {
            log::debug!("向 ssh-agent 索取身份失败:{error}");
            return Ok(false);
        }
    };
    if identities.is_empty() {
        log::debug!("ssh-agent 里没有身份");
        return Ok(false);
    }

    events.status(format!("正在用 ssh-agent 认证({} 个身份)…", identities.len()));
    for identity in &identities {
        let key = identity.public_key().into_owned();
        let fingerprint = key.fingerprint(Default::default()).to_string();
        match handle
            .authenticate_publickey_with(&params.user, key, rsa_hash, &mut agent)
            .await
        {
            Ok(result) if result.success() => return Ok(true),
            Ok(_) => {
                reason.clear();
                tried.push(format!("agent:{fingerprint}"));
            }
            Err(error) => {
                // 单个身份签名失败(比如智能卡被拔了)不该中断整条认证链。
                log::debug!("用 agent 身份 {fingerprint} 签名失败:{error}");
            }
        }
    }
    *reason = format!(
        "ssh-agent 里的 {} 个身份都被拒绝(该公钥可能没装到对端)",
        identities.len()
    );
    Ok(false)
}

/// 用私钥文件认证;成功返回 `true`。
///
/// `Err` 同上:只在传输层出问题时返回。
async fn try_key_files(
    handle: &mut client::Handle<ClientHandler>,
    params: &SshParams,
    rsa_hash: Option<HashAlg>,
    events: &EventSink,
    tried: &mut Vec<String>,
    reason: &mut String,
) -> Result<bool, SshError> {
    for path in key_paths(params) {
        // 没有口令的密钥才直接可用;加密的私钥需要用户给 passphrase,留待以后支持。
        let key = match load_secret_key(&path, None) {
            Ok(key) => key,
            Err(error) => {
                log::debug!("装载私钥 {} 失败:{error}", path.display());
                continue;
            }
        };
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());

        events.status(format!("正在用私钥 {name} 认证 …"));
        tried.push(format!("key:{name}"));
        let result = handle
            .authenticate_publickey(
                &params.user,
                PrivateKeyWithHashAlg::new(Arc::new(key), rsa_hash),
            )
            .await?;
        if result.success() {
            return Ok(true);
        }
        *reason = format!("私钥 {name} 被拒绝(对应的公钥可能没装到对端)");
    }
    Ok(false)
}

/// 要尝试的私钥文件:先是用户显式给的,再是 `~/.ssh` 下的默认名(存在的才列)。
fn key_paths(params: &SshParams) -> Vec<PathBuf> {
    let mut paths = params.key_files.clone();
    if let Some(ssh_dir) = dirs::home_dir().map(|home| home.join(".ssh")) {
        for name in DEFAULT_KEY_NAMES {
            let path = ssh_dir.join(name);
            if path.is_file() && !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    paths
}

/// 连接本机的 ssh-agent。
///
/// 两个平台找的是不同的东西:Unix 认 `SSH_AUTH_SOCK` 这个 unix socket;Windows 上可能是
/// 系统自带 OpenSSH 的命名管道,也可能是 Pageant(老牌 PuTTY 的 agent)。两者都探一遍,
/// 探不到就返回 `None`(不是错误 —— 很多机器就是没有 agent)。
async fn connect_agent() -> Option<Agent> {
    #[cfg(unix)]
    {
        match AgentClient::connect_env().await {
            Ok(client) => Some(client.dynamic()),
            Err(error) => {
                log::debug!("连接 ssh-agent(SSH_AUTH_SOCK)失败:{error}");
                None
            }
        }
    }

    #[cfg(windows)]
    {
        const OPENSSH_AGENT_PIPE: &str = r"\\.\pipe\openssh-ssh-agent";
        match AgentClient::connect_named_pipe(OPENSSH_AGENT_PIPE).await {
            Ok(client) => return Some(client.dynamic()),
            Err(error) => log::debug!("连接 Windows OpenSSH agent 失败:{error}"),
        }
        match AgentClient::connect_pageant().await {
            Ok(client) => Some(client.dynamic()),
            Err(error) => {
                log::debug!("连接 Pageant 失败:{error}");
                None
            }
        }
    }
}
