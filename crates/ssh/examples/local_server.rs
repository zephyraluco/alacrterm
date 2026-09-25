//! 一台**只用于本地验证**的 SSH 服务端:russh 的服务端 + 一个玩具 shell。
//!
//! 它有两个用途,所以放在 `examples/` 而不是测试文件里:
//!
//! 1. **集成测试**(`tests/roundtrip.rs`)用 `#[path]` 把它包进来当「被连的那一端」——
//!    本机 / CI 通常没有可连的 sshd,而「能编译」离「能连上」差着整个协议栈;
//! 2. **手工验收**:先起服务端
//!    ```text
//!    cargo run -p ssh --example local_server -- 2222
//!    ```
//!    再在 GUI(`cargo run -p alacrterm`)里建一条指向 `127.0.0.1:2222` 的会话记录,
//!    双击打开,在建连对话框里填用户 `tester` / 密码 `alacrterm-test`。
//!
//! 玩具 shell 的行为(刻意做得可预测,好让测试断言):
//! - 申请 PTY 时回 `PTY:<term>:<cols>x<rows>`;
//! - `window_change` 时回 `SIZE:<cols>x<rows>`;
//! - 申请 shell 时回一段横幅与提示符 `$ `;
//! - 敲进来的字节**逐字节回显**(模拟真 PTY 的回显),用户看到的就是自己敲的字;
//! - 回车后回 `REPLY:<刚才那一行>`;
//! - 那一行是 `exit` 时按 [`EXIT_CODE`] 结束会话。
//!
//! ⚠️ **主机密钥是固定的**(见 [`HOST_KEY`]):随机生成的话,每一轮都会变,`known_hosts`
//! 里上一次那条记录就会变成「主机密钥已变更」而直接把后续连接拒掉。连过之后
//! `known_hosts` 会稳定记下这一条,不会再打扰你。
//!
//! ⚠️ 手工验收走的是 GUI,主机密钥会按 TOFU 记进你**真正的** `~/.ssh/known_hosts`(与
//! `ssh` 同一份,语义也相同);集成测试各自用 `%TEMP%` 下的临时文件,不碰它。
//!
//! ## sftp 子系统
//!
//! 服务端**也**支持 `sftp`(只读,见 [`FsSession`]):列的是**服务端进程当前目录**下的
//! 真实文件。这不是玩具 shell 的一部分,而是为了让「远端文件管理器」也能被验证 ——
//! 客户端那边 [SshFs](ssh::SshFs) 走的是**另开一条连接**的 sftp 子系统。

#![allow(dead_code)] // 被测试 `#[path]` 包进来时 `main` 是多余的

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use russh::server::{Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use russh_sftp::protocol::{Attrs, File, FileAttributes, Handle, Name, Status, StatusCode, Version};
use tokio::net::TcpListener;

/// 唯一接受的密码(测试与手工验收共用)。
pub const PASSWORD: &str = "alacrterm-test";
/// 申请 shell 时先吐的横幅。
pub const BANNER: &str = "alacrterm 本地测试服务端\r\n";
/// 收到 `exit` 时上报的退出码。
pub const EXIT_CODE: u32 = 7;
/// 提示符(测试靠它判断「shell 起来了」)。
pub const PROMPT: &str = "$ ";

/// 测试用主机私钥 —— **故意公开**的测试密钥,只用于本机验证。
///
/// 对应公钥:`ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFNPJaQ3eagV+Kd7/0lUx8XiBk2HQaDQUfzXAJLGnkW2`。
/// 固定不变的原因见模块文档(随机密钥会让 `known_hosts` 里的记录每轮就失效)。
const HOST_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACBTTyWkN3moFfine/9JVMfF4gZNh0Gg0FH81wCSxp5FtgAAAKDP4nctz+J3
LQAAAAtzc2gtZWQyNTUxOQAAACBTTyWkN3moFfine/9JVMfF4gZNh0Gg0FH81wCSxp5Ftg
AAAEDfpdk8Ac3XGvrS5mbENUILkEOOEMQ3/ldStPUsshunZlNPJaQ3eagV+Kd7/0lUx8Xi
Bk2HQaDQUfzXAJLGnkW2AAAAGWFsYWNydGVybS1sb2NhbC10ZXN0LWhvc3QBAgME
-----END OPENSSH PRIVATE KEY-----";

/// 每连接一个的处理器:只有「当前行」这一点状态,用来识别 `exit`;
/// 另外留着已打开的会话通道,`sftp` 子系统要在它上面跑(见 [`Self::subsystem_request`])。
pub struct LocalServer {
    line: String,
    /// 已打开的会话通道(按 channel id 存;`sftp` 子系统要取走一个自己用)。
    channels: HashMap<ChannelId, Channel<Msg>>,
    /// 玩具 shell 所在的那条通道。**只有它**的数据才当键盘输入处理 ——
    /// russh 会把**所有**通道的数据都送到 [`russh::server::Handler::data`],
    /// 不限住的话 sftp 那条通道的协议包会被当成键盘输入回显回去。
    shell: Option<ChannelId>,
}

impl LocalServer {
    fn new() -> Self {
        Self {
            line: String::new(),
            channels: HashMap::new(),
            shell: None,
        }
    }
}

/// 服务端「拒绝认证」的统一写法。`proceed_with_methods: None` = 不提示客户端还能试什么,
/// 逼它自己按顺序试(我们的客户端本来就是这么做的,这样测试更严格)。
fn reject() -> Auth {
    Auth::Reject {
        proceed_with_methods: None,
        partial_success: false,
    }
}

impl russh::server::Server for LocalServer {
    type Handler = Self;

    fn new_client(&mut self, _peer: Option<SocketAddr>) -> Self {
        Self::new()
    }

    fn handle_session_error(&mut self, error: russh::Error) {
        // 会话错误往往就是断言失败的原因,别吞掉。
        eprintln!("[local_server] 会话出错:{error}");
    }
}

impl russh::server::Handler for LocalServer {
    type Error = russh::Error;

    /// 只认一个密码,用来验证「公钥全失败后回落密码」这条链。
    async fn auth_password(&mut self, _user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if password == PASSWORD {
            Auth::Accept
        } else {
            reject()
        })
    }

    /// 不接受公钥:强制走密码分支,顺带证明 agent / 私钥缺失时不会卡住。
    async fn auth_publickey(
        &mut self,
        _user: &str,
        _key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<Auth, Self::Error> {
        Ok(reject())
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        // 留一份通道句柄:`sftp` 子系统要把它变成一条字节流。
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    /// `sftp` 子系统:把通道交给只读的 [`FsSession`](见文件末尾),让客户端能列目录。
    ///
    /// 我们**不**在玩具 shell 那条通道上支持 sftp(客户端也不会这么用),
    /// 这条只是让「远端文件管理器」也有一条能被验证的路。
    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        eprintln!("[local_server] subsystem_request {name}");
        if name != "sftp" {
            session.channel_failure(channel)?;
            return Ok(());
        }
        let Some(channel) = self.channels.remove(&channel) else {
            session.channel_failure(channel)?;
            return Ok(());
        };
        session.channel_success(channel.id())?;
        russh_sftp::server::run(channel.into_stream(), FsSession::default()).await;
        Ok(())
    }

    /// 把客户端上报的 PTY 类型与尺寸回一条消息(测试据此断言 `request_pty` 到了)。
    async fn pty_request(
        &mut self,
        channel: ChannelId,
        term: &str,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        eprintln!("[local_server] pty_request term={term} {col_width}x{row_height}");
        session.data(
            channel,
            format!("PTY:{term}:{col_width}x{row_height}\r\n").into_bytes(),
        )?;
        Ok(())
    }

    /// 客户端改窗口尺寸时回一条带新尺寸的消息(测试据此断言 `window_change` 到了)。
    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        eprintln!("[local_server] window_change {col_width}x{row_height}");
        session.data(
            channel,
            format!("SIZE:{col_width}x{row_height}\r\n").into_bytes(),
        )?;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        eprintln!("[local_server] shell_request");
        self.shell = Some(channel);
        session.data(
            channel,
            format!("{BANNER}{PROMPT}").into_bytes(),
        )?;
        Ok(())
    }

    /// 玩具 shell:逐字节回显、回车后回 `REPLY:<行>`,`exit` 结束会话。
    ///
    /// ⚠️ 只管自己那条通道(见 [`LocalServer::shell`]):sftp 那条走的是
    /// [`Self::subsystem_request`] 里那条字节流。
    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if self.shell != Some(channel) {
            return Ok(());
        }
        let text = String::from_utf8_lossy(data).into_owned();
        eprintln!("[local_server] data {} 字节:{text:?}", data.len());
        // 先原样回显(包括控制字符),让客户端看到一个「真终端」的样子。
        session.data(channel, data.to_vec())?;

        for ch in text.chars() {
            match ch {
                '\r' | '\n' => {
                    let line = std::mem::take(&mut self.line);
                    session.data(channel, b"\r\n".to_vec())?;
                    if line.trim() == "exit" {
                        session.exit_status_request(channel, EXIT_CODE)?;
                        session.eof(channel)?;
                        session.close(channel)?;
                        return Ok(());
                    }
                    session.data(
                        channel,
                        format!("REPLY:{line}\r\n{PROMPT}").into_bytes(),
                    )?;
                }
                // 退格:只回显(玩具 shell 不做真正的行编辑)。
                '\u{7f}' | '\u{8}' => {}
                ch => self.line.push(ch),
            }
        }
        Ok(())
    }

    /// **exec 通道**(不申请 PTY,`ssh host cmd` 那种):回一条 `EXEC:<命令>` 就带退出码结束。
    ///
    /// 命令写成 `exit N` 时可以指定退出码 —— 让脚本能验「退出码真的透出来了」。
    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(data).trim().to_string();
        eprintln!("[local_server] exec_request {command:?}");
        session.data(channel, format!("EXEC:{command}\r\n").into_bytes())?;
        let code = command
            .strip_prefix("exit ")
            .and_then(|rest| rest.trim().parse().ok())
            .unwrap_or(EXIT_CODE);
        session.exit_status_request(channel, code)?;
        session.eof(channel)?;
        session.close(channel)?;
        Ok(())
    }
}

/// 起一台服务端,返回它绑定的端口。
///
/// 监听 `127.0.0.1:0`(端口交给系统分配),所以测试之间不会撞端口。
pub async fn serve() -> std::io::Result<u16> {
    let config = Arc::new(russh::server::Config {
        // 认证失败后的等待:测试里要快,生产上防爆破。
        auth_rejection_time: Duration::from_millis(10),
        auth_rejection_time_initial: Some(Duration::from_millis(0)),
        keys: vec![host_key()],
        ..Default::default()
    });

    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        let mut server = LocalServer::new();
        if let Err(error) = server.run_on_socket(config, &listener).await {
            eprintln!("[local_server] 退出:{error}");
        }
    });
    Ok(port)
}

/// 固定那把测试主机密钥。
fn host_key() -> russh::keys::PrivateKey {
    russh::keys::PrivateKey::from_openssh(HOST_KEY).expect("内嵌的测试主机密钥应当能解析")
}

// ---------------------------------------------------------------- 只读 SFTP 子系统

/// 只读的 SFTP 处理:列**服务端进程当前目录**下的真实文件。
///
/// 目的只是让「远端文件管理器」这条路能被端到端验证(读目录 / 判断是不是目录),
/// 所以刻意最小:只实现 `realpath` / `stat` / `lstat` / `opendir` / `readdir` / `close`,
/// 其余(`open` / `write` / `remove` …)一律回 `SSH_FX_OP_UNSUPPORTED`。
///
/// 路径解析按**服务端本机**语义(`std::fs`),不做 POSIX 归一化 —— 测试与手工验收
/// 都在同一台机器上,Windows 盘符路径照样能用。
#[derive(Default)]
struct FsSession {
    /// 已经列过一次的目录句柄:`readdir` 第二次起回 `SSH_FX_EOF`(协议约定靠它收尾)。
    listed: HashSet<String>,
}

impl russh_sftp::server::Handler for FsSession {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        _version: u32,
        _extensions: HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.listed.remove(&handle);
        Ok(ok(id))
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let resolved = resolve(&path);
        Ok(Name {
            id,
            files: vec![File::dummy(resolved.to_string_lossy().to_string())],
        })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let metadata = std::fs::metadata(resolve(&path)).map_err(|_| StatusCode::NoSuchFile)?;
        Ok(Attrs {
            id,
            attrs: attributes(&metadata),
        })
    }

    /// 玩具文件系统里没有符号链接,`lstat` 与 `stat` 同义。
    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        self.stat(id, path).await
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let dir = resolve(&path);
        if !dir.is_dir() {
            return Err(StatusCode::NoSuchFile);
        }
        Ok(Handle {
            id,
            handle: dir.to_string_lossy().to_string(),
        })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        // 第一次列完就记下,第二次回 EOF(客户端据此停止读取)。
        if !self.listed.insert(handle.clone()) {
            return Err(StatusCode::Eof);
        }
        let entries = std::fs::read_dir(&handle).map_err(|_| StatusCode::NoSuchFile)?;
        let files = entries
            .flatten()
            .filter_map(|entry| {
                let metadata = entry.metadata().ok()?;
                Some(File::new(
                    entry.file_name().to_string_lossy().to_string(),
                    attributes(&metadata),
                ))
            })
            .collect();
        Ok(Name { id, files })
    }
}

/// `SSH_FX_OK` 状态包。
fn ok(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: "Ok".to_string(),
        language_tag: "en-US".to_string(),
    }
}

/// 相对路径按服务端进程的当前目录解析(与真 sshd 的「相对家目录」不同,但足够验证)。
fn resolve(path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

/// 组装属性:**显式**写 `permissions`,因为 `From<&Metadata>` 只在 unix 上填它,
/// 而 Windows 客户端要靠 `permissions` 里的 `S_IFDIR` / `S_IFREG` 位判断是不是目录。
fn attributes(metadata: &std::fs::Metadata) -> FileAttributes {
    let mut attrs = FileAttributes::empty();
    attrs.size = Some(metadata.len());
    attrs.permissions = Some(if metadata.is_dir() { 0o040755 } else { 0o100644 });
    attrs.mtime = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_secs() as u32);
    attrs
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse().ok())
        .unwrap_or(2222);

    let config = Arc::new(russh::server::Config {
        keys: vec![host_key()],
        ..Default::default()
    });
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let actual = listener.local_addr()?.port();

    println!("本地测试 SSH 服务端已启动:127.0.0.1:{actual}");
    println!("  用户:tester    密码:{PASSWORD}");
    println!("  连接:在 GUI 里建一条指向 127.0.0.1:{actual} 的会话记录,双击后填上面的账号");
    println!("  玩具 shell:逐字节回显;回车回 REPLY:<行>;输入 exit 按码 {EXIT_CODE} 结束。");
    println!("  按 Ctrl-C 退出服务端。");

    let mut server = LocalServer::new();
    // `run_on_socket` 要到进程结束才返回;Ctrl-C 会直接终止进程,这正是我们想要的。
    server.run_on_socket(config, &listener).await
}
