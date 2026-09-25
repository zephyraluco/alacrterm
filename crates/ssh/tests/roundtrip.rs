//! 端到端往返测试:用 russh 的**服务端**在本地起一个真 SSH 服务,再用我们的客户端连它。
//!
//! 为什么值得这么写:本机(或 CI)通常没有可连的 sshd,「能编译」离「能连上」差着整个
//! 协议栈。这里用真握手 / 真认证 / 真 PTY 通道跑一遍,覆盖:
//!
//! - TCP → 密钥交换 → 主机密钥校验(TOFU 写入临时 `known_hosts`,不碰用户那份);
//! - 密码认证 → 开会话通道 → 申请 PTY → 申请 shell;
//! - 远端字节 → [`SshEvent::Data`];
//! - 本地字节 → 远端(服务端把它回显成 `REPLY:<行>`);
//! - 窗口尺寸变更(服务端回 `SIZE:<cols>x<rows>`);
//! - 远端退出码 → [`SshEvent::Closed::exit_status`]。
//!
//! 服务端本身在 `examples/local_server.rs` —— 那份同时也能手工跑起来玩(见它的模块文档)。

use std::path::PathBuf;
use std::time::Duration;

use futures::StreamExt as _;
use ssh::{HostKeyDecision, HostKeyPolicy, SshAuth, SshEvent, SshParams, SshSession, SshSize};

/// `examples/local_server.rs` 作为模块包进来,直接用它的常量与 `serve()`。
///
/// 这样「测试用的服务端」与「手工验证用的服务端」是**同一份代码**,不会各自漂移。
#[path = "../examples/local_server.rs"]
mod local_server;

use local_server::{BANNER, EXIT_CODE, PASSWORD, PROMPT};

// ---------------------------------------------------------------- 脚手架

/// 每个测试用独立的 `known_hosts` 临时文件 —— 绝不写用户真正的那一份。
fn temp_known_hosts(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("alacrterm-ssh-test-{name}-known_hosts"));
    let _ = std::fs::remove_file(&path);
    path
}

/// 连上本地测试服务端并返回会话 + 事件流。
async fn connect(
    port: u16,
    known_hosts: PathBuf,
    size: SshSize,
) -> (
    SshSession,
    futures::channel::mpsc::UnboundedReceiver<SshEvent>,
) {
    let mut params = SshParams::new("127.0.0.1", port, "tester");
    params.auth = SshAuth::Password(PASSWORD.to_string());
    params.host_key = HostKeyPolicy::AcceptNew;
    params.known_hosts = Some(known_hosts);

    let (tx, rx) = futures::channel::mpsc::unbounded();
    let session = SshSession::connect(params, size, tx).expect("启动会话线程");
    (session, rx)
}

/// 一直收事件,直到 `predicate` 满足或超时;把所有收到的字节拼起来给断言用。
///
/// 会话**异常**结束时直接 panic —— 那几乎总是断言失败的真实原因,藏着只会让报告更难读。
///
/// `answer` 是遇到主机密钥确认请求时的回答(界面弹窗那一步的替身);
/// 传 `None` 表示「不该有人来问」,真收到就 panic。
async fn collect_until(
    events: &mut futures::channel::mpsc::UnboundedReceiver<SshEvent>,
    timeout: Duration,
    answer: Option<HostKeyDecision>,
    mut done: impl FnMut(&str, &Option<u32>) -> bool,
) -> (String, Option<u32>) {
    let mut text = String::new();
    let mut exit_status = None;

    let deadline = tokio::time::Instant::now() + timeout;
    while let Ok(Some(event)) = tokio::time::timeout(
        deadline.saturating_duration_since(tokio::time::Instant::now()),
        events.next(),
    )
    .await
    {
        match event {
            SshEvent::Data(bytes) => {
                text.push_str(&String::from_utf8_lossy(&bytes));
                if done(&text, &exit_status) {
                    return (text, exit_status);
                }
            }
            SshEvent::Status(status) => {
                eprintln!("[client] {status}");
                if done(&text, &exit_status) {
                    return (text, exit_status);
                }
            }
            SshEvent::Closed {
                exit_status: code,
                reason,
            } => {
                if let Some(reason) = reason {
                    panic!("会话异常结束:{reason}(已收到:{text})");
                }
                exit_status = code;
                return (text, exit_status);
            }
            // `Ask` 策略才会走到这里:按调用方的要求回答(界面弹窗那一步的替身)。
            SshEvent::HostKeyPrompt(prompt) => match answer {
                Some(decision) => {
                    assert!(!prompt.fingerprint.is_empty(), "确认请求应当带上指纹");
                    prompt.respond(decision);
                }
                None => panic!("意外收到主机密钥确认请求:{prompt:?}"),
            },
            // 「已连上」只是通知,断言靠后面的实际字节。
            SshEvent::Connected => {}
        }
    }
    (text, exit_status)
}

/// 等到会话以**异常**结束,返回失败原因(不会有人来问主机密钥)。
async fn collect_failure(
    events: &mut futures::channel::mpsc::UnboundedReceiver<SshEvent>,
) -> String {
    collect_failure_answering(events, None).await
}

/// [`collect_failure`] 的通用版:`answer` 是遇到主机密钥确认请求时的回答。
async fn collect_failure_answering(
    events: &mut futures::channel::mpsc::UnboundedReceiver<SshEvent>,
    answer: Option<HostKeyDecision>,
) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while let Ok(Some(event)) = tokio::time::timeout(
        deadline.saturating_duration_since(tokio::time::Instant::now()),
        events.next(),
    )
    .await
    {
        match event {
            SshEvent::Closed {
                reason: Some(reason), ..
            } => return reason,
            SshEvent::HostKeyPrompt(prompt) => match answer {
                Some(decision) => prompt.respond(decision),
                None => panic!("意外收到主机密钥确认请求:{prompt:?}"),
            },
            _ => {}
        }
    }
    panic!("会话没有以失败结束");
}

// ---------------------------------------------------------------- 用例

/// 主路径:连接 → 密码认证 → PTY → shell → 收远端字节 → 发本地字节 → resize → 退出码。
#[tokio::test]
async fn connects_authenticates_and_round_trips_bytes() {
    let port = local_server::serve().await.expect("起本地服务端");
    let known_hosts = temp_known_hosts("roundtrip");
    let (session, mut events) = connect(port, known_hosts.clone(), SshSize::cells(80, 24)).await;

    // 远端先回 PTY 上报(pty_request 的回执)+ 横幅 + 提示符。
    let (text, _) = collect_until(&mut events, Duration::from_secs(10), None, |text, _| {
        text.contains("PTY:xterm-256color:80x24") && text.contains(PROMPT)
    })
    .await;
    assert!(
        text.contains("PTY:xterm-256color:80x24"),
        "PTY 请求没到服务端,收到:{text:?}"
    );
    assert!(text.contains(BANNER), "没收到横幅,收到:{text:?}");

    // 本地字节要能到远端:服务端回车后回 REPLY:<那一行>。
    session.write(b"hello from client\r");
    let (text, _) = collect_until(&mut events, Duration::from_secs(10), None, |text, _| {
        text.contains("REPLY:hello from client")
    })
    .await;
    assert!(text.contains("REPLY:hello from client"), "回执没回来:{text:?}");

    // resize 也要到远端。
    session.resize(SshSize::cells(120, 40));
    let (text, _) = collect_until(&mut events, Duration::from_secs(10), None, |text, _| {
        text.contains("SIZE:120x40")
    })
    .await;
    assert!(text.contains("SIZE:120x40"), "尺寸变更没到服务端:{text:?}");

    // 结束:远端退出码要透出来。
    session.write(b"exit\r");
    let (_, exit_status) = collect_until(&mut events, Duration::from_secs(10), None, |_, code| {
        code.is_some()
    })
    .await;
    assert_eq!(exit_status, Some(EXIT_CODE));

    // TOFU:首次连接把主机密钥记进了我们指定的 known_hosts。
    let recorded = std::fs::read_to_string(&known_hosts).expect("known_hosts 应已写入");
    assert!(
        recorded.contains("127.0.0.1"),
        "known_hosts 没记下主机:{recorded:?}"
    );
    let _ = std::fs::remove_file(&known_hosts);
}

/// TOFU 的第二次连接:密钥已在文件里 ⇒ 不新增条目(仍能连上)。
#[tokio::test]
async fn trusts_a_recorded_host_on_the_second_connection() {
    let port = local_server::serve().await.expect("起本地服务端");
    let known_hosts = temp_known_hosts("tofu");

    for _ in 0..2 {
        let (session, mut events) = connect(port, known_hosts.clone(), SshSize::cells(80, 24)).await;
        let (text, _) = collect_until(&mut events, Duration::from_secs(10), None, |text, _| {
            text.contains(BANNER)
        })
        .await;
        assert!(text.contains(BANNER), "第二次连接失败:{text:?}");
        session.disconnect();
    }

    let recorded = std::fs::read_to_string(&known_hosts).expect("known_hosts 应已写入");
    // ⚠️ 数**非空**行:russh 往空的已知主机文件里写第一条时会先补一个换行
    // (`learn_known_hosts_path` 判断「文件是否以 \n 结尾」时空文件 seek 失败 ⇒ 当成没有),
    // 所以文件开头会多一个空行。OpenSSH 会忽略空行,这里也一样。
    let entries = recorded.lines().filter(|line| !line.trim().is_empty());
    assert_eq!(entries.count(), 1, "同一个主机只该记一条:{recorded:?}");
    let _ = std::fs::remove_file(&known_hosts);
}

/// `Strict` 策略下,没见过的主机必须被拒绝 —— 这是中间人防护的底线。
#[tokio::test]
async fn strict_policy_rejects_an_unknown_host() {
    let port = local_server::serve().await.expect("起本地服务端");
    let known_hosts = temp_known_hosts("strict");

    let mut params = SshParams::new("127.0.0.1", port, "tester");
    params.auth = SshAuth::Password(PASSWORD.to_string());
    params.host_key = HostKeyPolicy::Strict;
    params.known_hosts = Some(known_hosts.clone());

    let (tx, mut events) = futures::channel::mpsc::unbounded();
    let _session = SshSession::connect(params, SshSize::cells(80, 24), tx).expect("启动会话线程");

    let reason = collect_failure(&mut events).await;
    assert!(
        reason.contains("known_hosts"),
        "失败原因应当说清是主机密钥的问题:{reason:?}"
    );
    assert!(
        !known_hosts.exists(),
        "strict 策略不该往 known_hosts 里写任何东西"
    );
}

/// `Ask`(默认)策略:用户回答「信任」后照样连上,并把密钥记进 known_hosts。
///
/// 这条覆盖界面弹窗那一步 —— 握手线程等的是 [`HostKeyPrompt::respond`],
/// 不回答就只有超时;这里顺便验证回答确实被认。
#[tokio::test]
async fn ask_policy_connects_after_the_user_trusts() {
    let port = local_server::serve().await.expect("起本地服务端");
    let known_hosts = temp_known_hosts("ask-accept");

    let mut params = SshParams::new("127.0.0.1", port, "tester");
    params.auth = SshAuth::Password(PASSWORD.to_string());
    // 不设 host_key = 用默认的 Ask。
    params.known_hosts = Some(known_hosts.clone());

    let (tx, mut events) = futures::channel::mpsc::unbounded();
    let session = SshSession::connect(params, SshSize::cells(80, 24), tx).expect("启动会话线程");

    let (text, _) = collect_until(
        &mut events,
        Duration::from_secs(10),
        Some(HostKeyDecision::Accept),
        |text, _| text.contains(BANNER),
    )
    .await;
    assert!(text.contains(BANNER), "信任之后应当连上:{text:?}");

    let recorded = std::fs::read_to_string(&known_hosts).expect("信任后 known_hosts 应已写入");
    assert!(
        recorded.contains("127.0.0.1"),
        "known_hosts 没记下主机:{recorded:?}"
    );

    let _ = std::fs::remove_file(&known_hosts);
    session.disconnect();
}

/// `Ask` 策略:用户回答「不信任」⇒ 连接被放弃,且**不写** known_hosts。
#[tokio::test]
async fn ask_policy_rejects_when_the_user_declines() {
    let port = local_server::serve().await.expect("起本地服务端");
    let known_hosts = temp_known_hosts("ask-reject");

    let mut params = SshParams::new("127.0.0.1", port, "tester");
    params.auth = SshAuth::Password(PASSWORD.to_string());
    params.known_hosts = Some(known_hosts.clone());

    let (tx, mut events) = futures::channel::mpsc::unbounded();
    let _session = SshSession::connect(params, SshSize::cells(80, 24), tx).expect("启动会话线程");

    let reason = collect_failure_answering(&mut events, Some(HostKeyDecision::Reject)).await;
    assert!(
        reason.contains("未被信任"),
        "失败原因应当说明是用户拒绝:{reason:?}"
    );
    assert!(
        !known_hosts.exists(),
        "拒绝之后不该往 known_hosts 里写任何东西"
    );
}

/// 远端文件系统(SFTP):另开一条连接、能取家目录、能列一层、能判断是不是目录。
///
/// 服务端的 sftp 子系统服务的是**它自己的当前目录**(见 `local_server::FsSession`),
/// 所以这里顺带验证了「列出来的条目确实来自远端」。
#[tokio::test]
async fn filesystem_lists_the_remote_directory() {
    use ssh::SshFs;

    let port = local_server::serve().await.expect("起本地服务端");
    let known_hosts = temp_known_hosts("fs-list");

    let mut params = SshParams::new("127.0.0.1", port, "tester");
    params.auth = SshAuth::Password(PASSWORD.to_string());
    params.host_key = HostKeyPolicy::AcceptNew;
    params.known_hosts = Some(known_hosts.clone());

    let fs = SshFs::new(params);
    // 第一次请求才建连接 ⇒ 这里覆盖了「懒连接 + 认证 + 打开 sftp 子系统」整条路。
    let home = fs.home_dir().await.expect("取家目录");
    assert!(!home.is_empty(), "家目录不该是空串");

    let entries = fs.list_dir(home.clone()).await.expect("列目录");
    assert!(!entries.is_empty(), "{home} 下不该一条都没有");
    // 排序约定:目录在前、名字不区分大小写。
    if let Some(first_file) = entries.iter().position(|entry| !entry.is_dir) {
        assert!(
            entries[..first_file].iter().all(|entry| entry.is_dir),
            "目录应当排在文件之前"
        );
    }

    assert!(fs.is_dir(home.clone()).await.expect("这个路径应当是目录"));
    let missing = format!("{home}/no-such-entry-1a2b3c");
    assert!(
        !fs.is_dir(missing).await.unwrap_or(false),
        "不存在的路径不该被判成目录"
    );

    let _ = std::fs::remove_file(&known_hosts);
}

/// [`SshFs`] 那条连接没有确认通道:未知主机必须**直接失败并说清怎么办**,不能干等。
#[tokio::test]
async fn filesystem_reports_when_it_has_no_prompt_channel() {
    use ssh::SshFs;
    let port = local_server::serve().await.expect("起本地服务端");
    let known_hosts = temp_known_hosts("fs-ask");

    let mut params = SshParams::new("127.0.0.1", port, "tester");
    params.auth = SshAuth::Password(PASSWORD.to_string());
    params.known_hosts = Some(known_hosts.clone());

    let error = match SshFs::new(params).home_dir().await {
        Ok(home) => panic!("未知主机不该连上,却拿到家目录 {home:?}"),
        Err(error) => error.to_string(),
    };
    assert!(
        error.contains("确认通道"),
        "应当提示没有确认通道:{error:?}"
    );
    let _ = std::fs::remove_file(&known_hosts);
}

/// 密码不对时要给出可读的失败原因,而不是静默挂住。
#[tokio::test]
async fn wrong_password_fails_with_a_reason() {
    let port = local_server::serve().await.expect("起本地服务端");
    let known_hosts = temp_known_hosts("badpass");

    let mut params = SshParams::new("127.0.0.1", port, "tester");
    params.auth = SshAuth::Password("definitely-not-it".to_string());
    // 这个用例只测认证失败 ⇒ 主机密钥用 accept-new,免得半路多一道确认。
    params.host_key = HostKeyPolicy::AcceptNew;
    params.known_hosts = Some(known_hosts.clone());

    let (tx, mut events) = futures::channel::mpsc::unbounded();
    let _session = SshSession::connect(params, SshSize::cells(80, 24), tx).expect("启动会话线程");

    let reason = collect_failure(&mut events).await;
    assert!(reason.contains("认证失败"), "失败原因不够具体:{reason:?}");
    let _ = std::fs::remove_file(&known_hosts);
}

/// 连不上时要带上「哪个地址、为什么」—— 最常见的失败就是端口写错 / 服务没起。
#[tokio::test]
async fn unreachable_host_reports_the_address() {
    // 绑一个端口再立刻放掉:该端口基本可以确定没人监听(不像固定端口可能被别的服务占)。
    let port = {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("占一个端口");
        listener.local_addr().expect("取本地地址").port()
    };

    let mut params = SshParams::new("127.0.0.1", port, "tester");
    params.known_hosts = Some(temp_known_hosts("refused"));
    let (tx, mut events) = futures::channel::mpsc::unbounded();
    let _session = SshSession::connect(params, SshSize::cells(80, 24), tx).expect("启动会话线程");

    let reason = collect_failure(&mut events).await;
    assert!(
        reason.contains(&format!("127.0.0.1:{port}")),
        "失败原因应当带上地址:{reason:?}"
    );
}
