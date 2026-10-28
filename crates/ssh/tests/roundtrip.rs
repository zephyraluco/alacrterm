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
//! - 远端退出码 → [`SshEvent::Closed::exit_status`];
//! - SFTP:列目录 / 元数据,以及上传 → 下载的往返与**单个文件写回**(`upload_one`,预览窗口的
//!   「保存」走的就是它)。
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
                reason: Some(reason),
                ..
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
    assert!(
        text.contains("REPLY:hello from client"),
        "回执没回来:{text:?}"
    );

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
        let (session, mut events) =
            connect(port, known_hosts.clone(), SshSize::cells(80, 24)).await;
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

/// 传输(上传 / 下载):把本地文件传上去,再下回来,内容要一模一样。
///
/// 同时覆盖:`Transfer` 的进度事件、`Done(Ok)` 收尾、`stat` 报出来的大小,以及
/// 「中间那条**第二条连接**」也真的能建起来(见 `crates/ssh/src/fs.rs` 的模块文档)。
///
/// ⚠️ 这里的「远端」就是本机(`local_server` 的服务端跑在同一个进程里),所以:
/// - 远端路径直接取 `temp_dir()`,测试数据不会落到仓库目录里;
/// - **源目录与目标目录必须分开**,否则上传会把源文件自己截断(同一个文件);
/// - 收尾用 `std::fs` 直接删 —— [`SshFs`] 不提供删除远端文件的接口(本次只做读 + 上传下载)。
#[tokio::test]
async fn filesystem_upload_download_round_trip() {
    use ssh::{RemoteEntry, SshFs};

    let port = local_server::serve().await.expect("起本地服务端");
    let known_hosts = temp_known_hosts("fs-transfer");

    let mut params = SshParams::new("127.0.0.1", port, "tester");
    params.auth = SshAuth::Password(PASSWORD.to_string());
    params.host_key = HostKeyPolicy::AcceptNew;
    params.known_hosts = Some(known_hosts.clone());

    let fs = SshFs::new(params);
    let _ = fs.home_dir().await.expect("取家目录");

    // 本地源目录 / 远端目标目录分开,名字保持独立。
    let name = "alacrterm-transfer-test.bin";
    let source_dir = std::env::temp_dir().join("alacrterm-transfer-src");
    let remote_dir_path = std::env::temp_dir().join("alacrterm-transfer-dst");
    let _ = std::fs::remove_dir_all(&source_dir);
    let _ = std::fs::remove_dir_all(&remote_dir_path);
    std::fs::create_dir_all(&source_dir).expect("建本地源目录");
    std::fs::create_dir_all(&remote_dir_path).expect("建远端目标目录");

    let remote_dir = remote_dir_path.to_string_lossy().to_string();
    let remote_file = format!("{remote_dir}/{name}");

    // 本地源文件:比一个分块(64 KiB)大,好让进度上报真的被累加 / 节流过。
    let source = source_dir.join(name);
    let payload: Vec<u8> = (0..(200 * 1024u32)).map(|i| (i % 251) as u8).collect();
    std::fs::write(&source, &payload).expect("写本地源文件");

    let mut upload = fs.upload(vec![source.clone()], remote_dir.clone());
    let uploaded = drive(&mut upload).await;
    assert!(uploaded.is_ok(), "上传应当成功:{uploaded:?}");

    // 远端真的有了,而且大小对得上。
    let meta = fs.stat(remote_file.clone()).await.expect("取远端元数据");
    assert!(!meta.is_dir, "刚传上去的应当是文件");
    assert_eq!(meta.size, payload.len() as u64, "远端大小应当与本地一致");

    // 列目录里也要带上它,并且 `size` 是真实的(界面靠它显示大小)。
    let entries: Vec<RemoteEntry> = fs.list_dir(remote_dir.clone()).await.expect("列目录");
    let listed = entries
        .iter()
        .find(|entry| entry.name == name)
        .expect("列表里应当有刚传上去的文件");
    assert_eq!(
        listed.size,
        payload.len() as u64,
        "列表里的大小应当是文件大小"
    );

    // 下回本地另一个路径,逐字节比对。
    let target = std::env::temp_dir().join("alacrterm-transfer-target.bin");
    let _ = std::fs::remove_file(&target);
    let mut download = fs.download_one(remote_file.clone(), target.clone());
    let downloaded = drive(&mut download).await;
    assert!(downloaded.is_ok(), "下载应当成功:{downloaded:?}");
    assert_eq!(
        std::fs::read(&target).expect("读回下载的文件"),
        payload,
        "下回来的内容应当与传上去的一模一样"
    );

    let _ = std::fs::remove_file(&target);
    let _ = std::fs::remove_dir_all(&source_dir);
    let _ = std::fs::remove_dir_all(&remote_dir_path);
    let _ = std::fs::remove_file(&known_hosts);
}

/// 目录上传:整棵子树都要过去(远端该建的目录要建出来)。
#[tokio::test]
async fn filesystem_uploads_a_directory_recursively() {
    use ssh::SshFs;

    let port = local_server::serve().await.expect("起本地服务端");
    let known_hosts = temp_known_hosts("fs-upload-dir");

    let mut params = SshParams::new("127.0.0.1", port, "tester");
    params.auth = SshAuth::Password(PASSWORD.to_string());
    params.host_key = HostKeyPolicy::AcceptNew;
    params.known_hosts = Some(known_hosts.clone());

    let fs = SshFs::new(params);
    let _ = fs.home_dir().await.expect("取家目录");

    // 本地源树 / 远端目标目录分开(见上一个测试的说明)。
    let local_root = std::env::temp_dir().join("alacrterm-upload-src");
    let remote_root_path = std::env::temp_dir().join("alacrterm-upload-dst");
    let _ = std::fs::remove_dir_all(&local_root);
    let _ = std::fs::remove_dir_all(&remote_root_path);
    std::fs::create_dir_all(local_root.join("nested")).expect("建本地目录");
    std::fs::create_dir_all(&remote_root_path).expect("建远端目标目录");
    std::fs::write(local_root.join("a.txt"), b"top level").expect("写 a.txt");
    std::fs::write(local_root.join("nested").join("b.txt"), b"nested level").expect("写 b.txt");

    let remote_root = remote_root_path.to_string_lossy().to_string();
    let mut upload = fs.upload(vec![local_root.clone()], remote_root.clone());
    let uploaded = drive(&mut upload).await;
    assert!(uploaded.is_ok(), "整棵目录上传应当成功:{uploaded:?}");

    // 远端那两层的文件都在,内容也对(远端就是本机,所以这里的断言是真的在查「远端」)。
    for (relative, expected) in [("a.txt", "top level"), ("nested/b.txt", "nested level")] {
        let remote = remote_root_path.join("alacrterm-upload-src").join(relative);
        let meta = fs
            .stat(remote.to_string_lossy().to_string())
            .await
            .expect("远端文件应当存在");
        assert_eq!(meta.size, expected.len() as u64, "{remote:?} 大小不对");
        assert_eq!(
            std::fs::read_to_string(&remote).expect("读远端文件"),
            expected,
            "{remote:?} 内容不对"
        );
    }

    // 收尾:远端那棵、本地那棵都删掉。
    let _ = std::fs::remove_dir_all(&remote_root_path);
    let _ = std::fs::remove_dir_all(&local_root);
    let _ = std::fs::remove_file(&known_hosts);
}

/// 单个文件写回(`upload_one`):按**指定路径**写,覆盖 / 截断同名远端文件。
///
/// 预览窗口的「保存」走的就是这条 —— 要写回的正是打开时那个远端路径,不能改名、也不能落到
/// 别的目录(那是 `upload` 的语义)。
#[tokio::test]
async fn filesystem_writes_back_a_single_file() {
    use ssh::SshFs;

    let port = local_server::serve().await.expect("起本地服务端");
    let known_hosts = temp_known_hosts("fs-write-back");

    let mut params = SshParams::new("127.0.0.1", port, "tester");
    params.auth = SshAuth::Password(PASSWORD.to_string());
    params.host_key = HostKeyPolicy::AcceptNew;
    params.known_hosts = Some(known_hosts.clone());

    let fs = SshFs::new(params);
    let _ = fs.home_dir().await.expect("取家目录");

    // 本地那份(界面上对应「缓存文件」)/ 远端目标目录分开(见第一个传输测试的说明)。
    let local_dir = std::env::temp_dir().join("alacrterm-writeback-src");
    let remote_dir_path = std::env::temp_dir().join("alacrterm-writeback-dst");
    let _ = std::fs::remove_dir_all(&local_dir);
    let _ = std::fs::remove_dir_all(&remote_dir_path);
    std::fs::create_dir_all(&local_dir).expect("建本地目录");
    std::fs::create_dir_all(&remote_dir_path).expect("建远端目标目录");

    let local = local_dir.join("notes.txt");
    let edited = "改过之后的内容\n";
    std::fs::write(&local, edited).expect("写本地文件");

    // 远端先摆一份**更长**的旧内容:写回必须把多出来的那截截掉(`create` 是
    // CREATE|TRUNCATE|WRITE,不然文件尾部会留上一版的内容)。
    let remote_path = remote_dir_path.join("notes.txt");
    std::fs::write(&remote_path, "旧的、比新内容长得多的内容\n".repeat(64)).expect("写远端旧文件");
    let remote = remote_path.to_string_lossy().to_string();

    let mut write_back = fs.upload_one(local.clone(), remote.clone());
    let written = drive(&mut write_back).await;
    assert!(written.is_ok(), "写回应当成功:{written:?}");

    assert_eq!(
        std::fs::read_to_string(&remote_path).expect("读远端文件"),
        edited,
        "远端内容应当是写回的那一份"
    );
    let meta = fs.stat(remote.clone()).await.expect("取远端元数据");
    assert_eq!(meta.size, edited.len() as u64, "远端大小应当是写回后的大小");

    // 目标目录不存在时要给一句可读的失败,而不是静默成功。
    let missing = format!(
        "{}/no-such-dir-1a2b3c/notes.txt",
        std::env::temp_dir().to_string_lossy().replace('\\', "/")
    );
    let mut failed = fs.upload_one(local, missing);
    let message = drive(&mut failed)
        .await
        .expect_err("往不存在的目录里写应当失败")
        .to_string();
    assert!(
        message.contains("远端没有这个路径"),
        "失败原因应当可读,实际:{message}"
    );

    let _ = std::fs::remove_dir_all(&local_dir);
    let _ = std::fs::remove_dir_all(&remote_dir_path);
    let _ = std::fs::remove_file(&known_hosts);
}

/// 临时工作目录(绝对路径;服务端的 `resolve` 对绝对路径原样用)。
fn temp_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("alacrterm-ssh-test-{name}"));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("建临时工作目录");
    path
}

/// 新建 / 改名 / 删除:文件管理器工具栏那几枚按钮走的四条路。
///
/// - `mkdir` / `create_file` / `rename` 是**单次往返**的短操作(走浏览那条请求 / 应答);
/// - `remove` 走**传输**那条通道(目录要递归、可能有进度与取消)。
#[tokio::test]
async fn filesystem_creates_renames_and_removes() {
    use ssh::SshFs;

    let port = local_server::serve().await.expect("起本地服务端");
    let known_hosts = temp_known_hosts("fs-edit");

    let mut params = SshParams::new("127.0.0.1", port, "tester");
    params.auth = SshAuth::Password(PASSWORD.to_string());
    params.host_key = HostKeyPolicy::AcceptNew;
    params.known_hosts = Some(known_hosts.clone());

    let fs = SshFs::new(params);
    let _ = fs.home_dir().await.expect("取家目录");

    let root = temp_dir("fs-edit-work");
    // 服务端那边的路径用 `/` 分隔(它只把绝对路径原样交给 std::fs,Windows 也吃得下 `/`)。
    let at = |name: &str| format!("{}/{}", root.to_string_lossy().replace('\\', "/"), name);

    // ---- 新建文件夹 ----
    let dir = at("logs");
    fs.mkdir(dir.clone()).await.expect("建目录");
    assert!(
        fs.is_dir(dir.clone()).await.expect("看新建的目录"),
        "mkdir 之后它应当是个目录"
    );

    // 再建一次要失败(而不是静默成功)—— 界面那边靠这条错误提示。
    let again = fs.mkdir(dir.clone()).await;
    assert!(again.is_err(), "同名目录再建一次应当失败:{again:?}");

    // ---- 新建空文件 ----
    let file = at("notes.txt");
    fs.create_file(file.clone()).await.expect("建空文件");
    let meta = fs.stat(file.clone()).await.expect("取新建文件元数据");
    assert!(!meta.is_dir, "新建的应当是文件");
    assert_eq!(meta.size, 0, "新建的文件应当是空的");

    // 已存在时也要失败(不然会把用户的东西清空)。
    assert!(
        fs.create_file(file.clone()).await.is_err(),
        "同名文件再建一次应当失败"
    );

    // ---- 改名 ----
    let renamed = at("renamed.txt");
    fs.rename(file.clone(), renamed.clone())
        .await
        .expect("改名");
    assert!(
        fs.stat(renamed.clone()).await.is_ok(),
        "改名之后新名字应当存在"
    );
    assert!(fs.stat(file).await.is_err(), "改名之后旧名字不该还在");

    // ---- 删除一个文件 ----
    let mut remove_file = fs.remove(renamed.clone(), false);
    drive(&mut remove_file).await.expect("删文件");
    assert!(fs.stat(renamed).await.is_err(), "删完不该还在");

    // ---- 递归删一棵树 ----
    let tree = at("tree");
    fs.mkdir(tree.clone()).await.expect("建树根");
    for nested in ["tree/one.txt", "tree/two.txt"] {
        fs.create_file(at(nested)).await.expect("建树里的文件");
    }
    fs.mkdir(at("tree/deep")).await.expect("建子目录");
    fs.create_file(at("tree/deep/three.txt"))
        .await
        .expect("建子目录里的文件");

    let mut remove_tree = fs.remove(tree.clone(), true);
    drive(&mut remove_tree).await.expect("递归删目录");
    assert!(
        fs.stat(tree).await.is_err(),
        "递归删除之后整棵树都不该在(包括子目录里的文件)"
    );

    // ---- 删一个不存在的东西:要给一句可读的失败 ----
    let mut missing = fs.remove(at("gone"), false);
    let message = drive(&mut missing)
        .await
        .expect_err("删不存在的路径应当失败")
        .to_string();
    println!("错误:{message}");
    assert!(
        message.contains("远端没有这个路径"),
        "失败原因应当可读,实际:{message}"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_file(&known_hosts);
}

/// 取消:请求取消之后,`Done` 要给 [`SshError::Cancelled`],而不是装作成功。
#[tokio::test]
async fn cancelled_transfer_reports_cancelled() {
    use ssh::{SshError, SshFs};

    let port = local_server::serve().await.expect("起本地服务端");
    let known_hosts = temp_known_hosts("fs-cancel");

    let mut params = SshParams::new("127.0.0.1", port, "tester");
    params.auth = SshAuth::Password(PASSWORD.to_string());
    params.host_key = HostKeyPolicy::AcceptNew;
    params.known_hosts = Some(known_hosts.clone());

    let fs = SshFs::new(params);
    let _ = fs.home_dir().await.expect("取家目录");

    // 源目录 / 目标目录分开(见第一个传输测试的说明)。
    let source_dir = std::env::temp_dir().join("alacrterm-cancel-src");
    let remote_dir_path = std::env::temp_dir().join("alacrterm-cancel-dst");
    let _ = std::fs::remove_dir_all(&source_dir);
    let _ = std::fs::remove_dir_all(&remote_dir_path);
    std::fs::create_dir_all(&source_dir).expect("建本地源目录");
    std::fs::create_dir_all(&remote_dir_path).expect("建远端目标目录");

    // 够大,保证「取消」一定发生在传完之前:开传之前就已经置位了,所以第一个分块边界就会停。
    let source = source_dir.join("alacrterm-cancel-test.bin");
    std::fs::write(&source, vec![7u8; 8 * 1024 * 1024]).expect("写本地源文件");

    let remote_dir = remote_dir_path.to_string_lossy().to_string();
    let mut upload = fs.upload(vec![source.clone()], remote_dir.clone());
    upload.cancel.cancel();
    let result = drive(&mut upload).await;
    assert!(
        matches!(result, Err(SshError::Cancelled)),
        "取消之后应当报 Cancelled,实际是 {result:?}"
    );

    let _ = std::fs::remove_dir_all(&source_dir);
    let _ = std::fs::remove_dir_all(&remote_dir_path);
    let _ = std::fs::remove_file(&known_hosts);
}

/// 把一条传输跑到结束,返回 `Done` 里的结果。
///
/// 顺带断言:进度里的字节数是**单调不减**的、不超过总数,文件数也不超过总数 —— 界面上的
/// 进度条与「第 n/m 个」就靠这几条不跳。
async fn drive(transfer: &mut ssh::Transfer) -> Result<(), ssh::SshError> {
    use ssh::TransferEvent;

    let mut last_bytes = 0;
    while let Some(event) = transfer.events.next().await {
        match event {
            TransferEvent::Progress(progress) => {
                assert!(
                    progress.bytes_done >= last_bytes,
                    "进度字节数不该倒退:{} -> {}",
                    last_bytes,
                    progress.bytes_done
                );
                assert!(
                    progress.bytes_total == 0 || progress.bytes_done <= progress.bytes_total,
                    "已传字节数不该超过总数:{progress:?}"
                );
                assert!(
                    progress.files_done <= progress.files_total,
                    "已传文件数不该超过总数(目录不能算进 files_done):{progress:?}"
                );
                last_bytes = progress.bytes_done;
            }
            TransferEvent::Done(result) => return result,
        }
    }
    panic!("通道在 Done 之前就断了");
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
    assert!(error.contains("确认通道"), "应当提示没有确认通道:{error:?}");
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

/// A single offered algorithm forces negotiation to use it, rather than silently
/// succeeding without compression. Exercise both directions and the SFTP connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compression_algorithms_round_trip_shell_and_sftp() {
    use russh::compression::{NONE, ZLIB, ZLIB_LEGACY};
    for (index, algorithm) in [ZLIB_LEGACY, ZLIB, NONE].into_iter().enumerate() {
        let port = local_server::serve_with_compression(vec![algorithm].into())
            .await
            .unwrap();
        let known_hosts = temp_known_hosts(&format!("compression-{index}"));
        let (session, mut events) =
            connect(port, known_hosts.clone(), SshSize::cells(80, 24)).await;
        let (banner, _) = collect_until(&mut events, Duration::from_secs(10), None, |text, _| {
            text.contains(PROMPT)
        })
        .await;
        assert!(
            banner.contains(BANNER),
            "shell did not start with {algorithm:?}"
        );
        let payload = "compressible-data-".repeat(1024);
        session.write(format!("{payload}\n").as_bytes());
        let expected = format!("REPLY:{payload}");
        let (reply, _) = collect_until(&mut events, Duration::from_secs(10), None, |text, _| {
            text.contains(&expected)
        })
        .await;
        assert!(
            reply.contains(&expected),
            "payload round trip failed with {algorithm:?}"
        );
        let fs = session.fs();
        let home = fs.home_dir().await.unwrap();
        fs.list_dir(home).await.unwrap();
        session.disconnect();
        let _ = std::fs::remove_file(known_hosts);
    }
}

#[tokio::test]
async fn completion_metadata_reads_are_bounded_and_do_not_block_browsing() {
    let port = local_server::serve().await.unwrap();
    let known_hosts = temp_known_hosts("completion-metadata");
    let mut params = SshParams::new("127.0.0.1", port, "tester").with_password(PASSWORD);
    params.host_key = HostKeyPolicy::AcceptNew;
    params.known_hosts = Some(known_hosts.clone());
    let fs = ssh::SshFs::new(params);
    let root = known_hosts.with_extension("metadata");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("package.json"),
        b"{\"scripts\":{\"build\":\"never execute\"}}",
    )
    .unwrap();
    std::fs::write(root.join("large"), vec![b'x'; 256 * 1024 + 1]).unwrap();
    let bytes = tokio::time::timeout(
        Duration::from_secs(10),
        fs.read_completion_file(root.join("package.json").to_string_lossy()),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(bytes.starts_with(b"{\"scripts\":"));
    assert!(
        fs.read_completion_file(root.join("large").to_string_lossy())
            .await
            .is_err()
    );
    assert_eq!(
        fs.list_completion_dir(root.to_string_lossy())
            .await
            .unwrap()
            .len(),
        2
    );
    assert!(fs.home_dir().await.is_ok());
    std::fs::remove_dir_all(root).unwrap();
    let _ = std::fs::remove_file(known_hosts);
}

#[tokio::test]
async fn native_login_preserves_banner_and_early_input_when_activation_is_refused() {
    let port = local_server::serve().await.unwrap();
    let known_hosts = temp_known_hosts("login-input-order");
    let mut params = SshParams::new("127.0.0.1", port, "tester").with_password(PASSWORD);
    params.host_key = HostKeyPolicy::AcceptNew;
    params.known_hosts = Some(known_hosts.clone());
    let (sender, mut events) = futures::channel::mpsc::unbounded();
    let session = SshSession::connect_with_setup(
        params,
        SshSize::cells(80, 24),
        sender,
        Some(std::sync::Arc::new(ProbeEnvironment)),
    )
    .unwrap();
    session.write(b"first\nsecond\n");
    let (text, _) = collect_until(&mut events, Duration::from_secs(9), None, |text, _| {
        text.contains("REPLY:second")
    })
    .await;
    assert!(text.contains(BANNER));
    assert!(text.find(BANNER).unwrap() < text.find("REPLY:first").unwrap());
    assert!(text.find("REPLY:first").unwrap() < text.find("REPLY:second").unwrap());
    assert!(!text.contains("never injected"));
    session.disconnect();
    let _ = std::fs::remove_file(known_hosts);
}

#[tokio::test]
async fn refused_activation_preserves_plain_interactive_shell() {
    let port = local_server::serve().await.unwrap();
    let known_hosts = temp_known_hosts("integration-fallback");
    let mut params = SshParams::new("127.0.0.1", port, "tester").with_password(PASSWORD);
    params.host_key = HostKeyPolicy::AcceptNew;
    params.known_hosts = Some(known_hosts.clone());
    let (sender, mut events) = futures::channel::mpsc::unbounded();
    let session = SshSession::connect_with_setup(
        params,
        SshSize::cells(80, 24),
        sender,
        Some(std::sync::Arc::new(ProbeEnvironment)),
    )
    .unwrap();
    let (text, _) = collect_until(&mut events, Duration::from_secs(10), None, |text, _| {
        text.contains(PROMPT)
    })
    .await;
    assert!(text.contains(BANNER));
    assert!(!text.contains("ALACRTERM_SHELL"));
    session.disconnect();
    let _ = std::fs::remove_file(known_hosts);
}

#[tokio::test]
async fn dropping_a_session_cancels_an_unresponsive_ssh_handshake() {
    use tokio::io::AsyncReadExt as _;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let (sender, mut events) = futures::channel::mpsc::unbounded();
    let session = SshSession::connect(
        SshParams::new("127.0.0.1", port, "user"),
        SshSize::cells(80, 24),
        sender,
    )
    .unwrap();
    let (mut socket, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
        .await
        .unwrap()
        .unwrap();
    drop(session);
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(event) = events.next().await {
            if matches!(event, SshEvent::Closed { .. }) {
                return;
            }
        }
        panic!("session ended without a close event");
    })
    .await
    .unwrap();
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), socket.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn dropping_a_session_cancels_a_pending_host_key_confirmation() {
    let port = local_server::serve().await.unwrap();
    let mut params = SshParams::new("127.0.0.1", port, "tester");
    params.known_hosts = Some(temp_known_hosts("cancel-host-key"));
    let (sender, mut events) = futures::channel::mpsc::unbounded();
    let session = SshSession::connect(params, SshSize::cells(80, 24), sender).unwrap();
    let prompt = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(SshEvent::HostKeyPrompt(prompt)) = events.next().await {
                break prompt;
            }
        }
    })
    .await
    .unwrap();
    drop(session);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match events.next().await {
                Some(SshEvent::Closed { .. }) => break,
                Some(SshEvent::Connected) => panic!("cancelled connection became running"),
                None => panic!("missing close event"),
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    drop(prompt);
}

#[tokio::test]
async fn file_transfer_survives_closing_its_terminal_connection() {
    let port = local_server::serve().await.unwrap();
    let known_hosts = temp_known_hosts("transfer-owner");
    let (session, mut events) = connect(port, known_hosts.clone(), SshSize::cells(80, 24)).await;
    collect_until(&mut events, Duration::from_secs(5), None, |text, _| {
        text.contains(PROMPT)
    })
    .await;
    let fs = session.fs();
    let root = std::env::temp_dir().join(format!(
        "alacrterm-transfer-owner-{}-{port}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("source.bin");
    let destination = root.join("destination.bin");
    let contents = vec![42; 512 * 1024];
    std::fs::write(&source, &contents).unwrap();
    let mut upload = fs.upload_one(&source, destination.to_string_lossy().into_owned());
    drop(session);
    drive(&mut upload).await.unwrap();
    assert_eq!(std::fs::read(&destination).unwrap(), contents);
    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_file(known_hosts);
}

struct ProbeEnvironment;
impl ssh::SessionSetup for ProbeEnvironment {
    fn prepare<'a>(
        &'a self,
        mut session: ssh::SessionContext<'a>,
    ) -> futures::future::BoxFuture<'a, ()> {
        Box::pin(async move {
            assert!(
                !session
                    .set_env("TEST_SESSION_SETUP", "1", Duration::from_secs(2))
                    .await
                    .unwrap()
            );
        })
    }
}

#[tokio::test]
async fn host_supplied_x11_target_receives_authenticated_setup_and_requests() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    struct Target(Arc<AtomicBool>);
    struct Lease(Arc<AtomicBool>);
    impl ssh::X11Target for Target {
        fn acquire(&self) -> anyhow::Result<Box<dyn ssh::X11Lease>> {
            Ok(Box::new(Lease(self.0.clone())))
        }
    }
    impl ssh::X11Lease for Lease {
        fn connect(
            &self,
        ) -> futures::future::BoxFuture<'_, anyhow::Result<Box<dyn ssh::X11Stream>>> {
            Box::pin(async move {
                use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
                self.0.store(true, Ordering::SeqCst);
                let (client, mut server) = tokio::io::duplex(4096);
                tokio::spawn(async move {
                    let mut header = [0; 12];
                    server.read_exact(&mut header).await.unwrap();
                    assert_eq!(header, [b'l', 0, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
                    let mut body = vec![0; 72];
                    body[20] = 1;
                    server.write_all(&[1, 0, 11, 0, 0, 0, 18, 0]).await.unwrap();
                    server.write_all(&body).await.unwrap();
                    let mut requests = [0; 48];
                    server.read_exact(&mut requests).await.unwrap();
                    assert_eq!(requests[44], 43);
                    let mut reply = [0; 32];
                    reply[0] = 1;
                    reply[2] = 3;
                    server.write_all(&reply).await.unwrap();
                });
                Ok(Box::new(client) as Box<dyn ssh::X11Stream>)
            })
        }
    }
    let port = local_server::serve().await.unwrap();
    let known_hosts = temp_known_hosts("generic-x11");
    let mut params = SshParams::new("127.0.0.1", port, "tester").with_password(PASSWORD);
    params.host_key = HostKeyPolicy::AcceptNew;
    params.known_hosts = Some(known_hosts.clone());
    let connected = Arc::new(AtomicBool::new(false));
    let (sender, mut events) = futures::channel::mpsc::unbounded();
    let options = ssh::SessionOptions {
        x11: Some(Arc::new(Target(connected.clone()))),
        ..Default::default()
    };
    let session =
        SshSession::connect_with_options(params, SshSize::cells(80, 24), sender, options).unwrap();
    let (text, _) = collect_until(&mut events, Duration::from_secs(10), None, |text, _| {
        text.contains("X11:OK") || text.contains("X11:FAILED")
    })
    .await;
    assert!(text.contains("X11:OK"), "{text}");
    assert!(connected.load(Ordering::SeqCst));
    session.disconnect();
    let _ = std::fs::remove_file(known_hosts);
}

#[tokio::test]
async fn host_setup_executes_auxiliary_commands_without_replacing_native_shell() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    struct Setup(Arc<AtomicBool>);
    impl ssh::SessionSetup for Setup {
        fn prepare<'a>(
            &'a self,
            session: ssh::SessionContext<'a>,
        ) -> futures::future::BoxFuture<'a, ()> {
            Box::pin(async move {
                let helper = session.connect_auxiliary().await.unwrap();
                let output = helper
                    .exec("exit 7", Duration::from_secs(2), 1024)
                    .await
                    .unwrap();
                assert_eq!(output.exit_status, Some(7));
                assert!(output.stdout.starts_with(b"EXEC:exit 7"));
                assert!(
                    helper
                        .exec("exit 0", Duration::from_secs(2), 4)
                        .await
                        .is_err()
                );
                helper.disconnect().await.unwrap();
                self.0.store(true, Ordering::SeqCst);
            })
        }
    }
    let port = local_server::serve().await.unwrap();
    let known_hosts = temp_known_hosts("generic-setup");
    let mut params = SshParams::new("127.0.0.1", port, "tester").with_password(PASSWORD);
    params.host_key = HostKeyPolicy::AcceptNew;
    params.known_hosts = Some(known_hosts.clone());
    let ready = Arc::new(AtomicBool::new(false));
    let (sender, mut events) = futures::channel::mpsc::unbounded();
    let session = SshSession::connect_with_setup(
        params,
        SshSize::cells(80, 24),
        sender,
        Some(Arc::new(Setup(ready.clone()))),
    )
    .unwrap();
    let (text, _) = collect_until(&mut events, Duration::from_secs(10), None, |text, _| {
        text.ends_with(PROMPT)
    })
    .await;
    assert!(text.contains(BANNER));
    assert!(ready.load(Ordering::SeqCst));
    session.disconnect();
    let _ = std::fs::remove_file(known_hosts);
}

#[tokio::test]
async fn closing_a_session_cancels_pending_host_preparation() {
    struct Setup(std::sync::Arc<tokio::sync::Notify>);
    impl ssh::SessionSetup for Setup {
        fn prepare<'a>(
            &'a self,
            _session: ssh::SessionContext<'a>,
        ) -> futures::future::BoxFuture<'a, ()> {
            Box::pin(async move {
                self.0.notify_one();
                std::future::pending().await
            })
        }
    }
    let port = local_server::serve().await.unwrap();
    let known_hosts = temp_known_hosts("cancel-setup");
    let mut params = SshParams::new("127.0.0.1", port, "tester").with_password(PASSWORD);
    params.host_key = HostKeyPolicy::AcceptNew;
    params.known_hosts = Some(known_hosts.clone());
    let (sender, mut events) = futures::channel::mpsc::unbounded();
    let started = std::sync::Arc::new(tokio::sync::Notify::new());
    let session = SshSession::connect_with_setup(
        params,
        SshSize::cells(80, 24),
        sender,
        Some(std::sync::Arc::new(Setup(started.clone()))),
    )
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    session.disconnect();
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(event) = events.next().await {
            if let SshEvent::Closed { reason, .. } = event {
                assert!(reason.is_none());
                return;
            }
        }
        panic!("missing close event");
    })
    .await
    .unwrap();
    let _ = std::fs::remove_file(known_hosts);
}
