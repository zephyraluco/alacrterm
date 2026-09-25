//! SSH 客户端:基于 [russh] 的**远端 shell 通道**,给终端提供「一对方位字节流 + 尺寸变更」。
//!
//! 它不关心终端模拟器:输入是一串字节(键盘)、输出也是一串字节(远端 PTY 的原样输出),
//! 中间那层 SSH 协议(握手 / 认证 / 加密 / 信道流控)全在这里([`session`])。
//! 调用方(见 `crates/terminal` 的 SSH 后端)只需要:
//!
//! 1. [`SshSession::connect`] 建连(交互式 shell)或 [`SshSession::connect_exec`] 跑单条命令,
//!    并拿到一个事件流([`SshEvent`]);
//! 2. 把键盘字节交 [`SshSession::write`]、把窗口变化交 [`SshSession::resize`];
//! 3. 事件流里的 [`SshEvent::Data`] 原样喂给终端模拟器,[`SshEvent::Closed`] 结束会话。
//!
//! ## 运行时
//!
//! russh 建在 **tokio** 上,而宿主(终端 / gpui)用的是自己的 futures 执行器,两者不能混。
//! 所以本 crate **自带一个 OS 线程**,线程内跑一个 current-thread tokio 运行时,把 SSH
//! 的异步世界关在里面;跨边界只用两个**执行器无关**的通道:
//!
//! - 命令(写数据 / 改尺寸 / 断开):`tokio::sync::mpsc::UnboundedSender`,普通线程也能 `send`;
//! - 事件(远端数据 / 结束):调用方传进来的 `futures::channel::mpsc::UnboundedSender`,
//!   它就是个普通 future,任何执行器都能 `next().await`。
//!
//! 这样宿主不需要引入 tokio,`crates/ssh` 也不需要知道 gpui 的存在。
//!
//! ## 认证
//!
//! 按 OpenSSH 的习惯**依次尝试**,任一成功即停(见 [`auth`]):
//! ssh-agent → 私钥文件([`SshParams::key_files`] 指定的,再是 `~/.ssh/id_{ed25519,ecdsa,rsa}`)
//! → 密码(仅当 [`SshAuth::Password`] 给了密码)。
//!
//! ## 主机密钥
//!
//! 默认 [`HostKeyPolicy::Ask`](params::HostKeyPolicy::Ask)(与 `ssh -o StrictHostKeyChecking=ask` 同):
//! **没见过的**主机把指纹当 [`SshEvent::HostKeyPrompt`](session::SshEvent::HostKeyPrompt) 交给宿主,
//! 由用户决定要不要信任(同意才记进 `~/.ssh/known_hosts`,之后按 TOFU 比对);
//! **密钥变了**一律拒绝(防中间人),返回 [`SshError::HostKeyChanged`],绝不自动覆盖。
//!
//! 宿主必须在 [`HOST_KEY_TIMEOUT`] 内回答 —— 这段等待发生在握手路径上,不回答就放弃连接。
//!
//! ## 远端文件系统
//!
//! [`SshFs`](crate::SshFs) 另开一条连接走 SFTP 子系统(见 [`fs`] 模块文档里的取舍),
//! 由 [`SshSession::fs`] 取到,用于远端目录浏览。它是**懒连接**:不用就不连。

mod auth;
mod error;
mod fs;
mod handler;
mod params;
mod session;

pub use error::SshError;
pub use fs::{RemoteEntry, SshFs};
pub use handler::{HOST_KEY_TIMEOUT, HostKeyDecision, HostKeyPrompt};
pub use params::{DEFAULT_TERM, HostKeyPolicy, SshAuth, SshParams, SshSize};
pub use session::{SshEvent, SshSession};
