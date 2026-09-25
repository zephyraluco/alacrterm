//! 对**真实主机**的远端文件系统测试(默认不跑)。
//!
//! ```text
//! $env:SSH_TEST_HOST='172.19.9.66'; $env:SSH_TEST_USER='jz'; $env:SSH_TEST_PASSWORD='...'
//! cargo test -p ssh --test remote_fs -- --ignored --nocapture
//! ```
//!
//! 凭据从环境变量读,**不落盘、不进仓库**;`tests/roundtrip.rs` 那套本地服务端覆盖不了
//! 这一段——它的玩具 shell 没有 sftp 子系统,而 SFTP 的握手 / 状态码 / 路径语义都只有真
//! 服务端才验得到。

use std::time::Duration;

use ssh::{SshAuth, SshFs, SshParams};

/// 从环境变量取参数;缺任何一项就跳过(返回 `None`)。
///
/// 之所以不 panic:CI 上没有这台机器,`--ignored` 之外也不会跑到这里。
fn params() -> Option<SshParams> {
    let host = std::env::var("SSH_TEST_HOST").ok()?;
    let user = std::env::var("SSH_TEST_USER").ok()?;
    let password = std::env::var("SSH_TEST_PASSWORD").ok()?;
    let port = std::env::var("SSH_TEST_PORT")
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(22);

    let mut params = SshParams::new(host, port, user);
    params.auth = SshAuth::Password(password);
    // 首次连接按 TOFU 记进**默认的** known_hosts(与 `ssh` 同一份,语义也相同)。
    params.host_key = ssh::HostKeyPolicy::AcceptNew;
    Some(params)
}

/// 带超时地等一个 future,超时直接失败 —— 真机测试最糟的失败方式是「挂住不返回」。
async fn with_timeout<T>(what: &str, future: impl Future<Output = T>) -> T {
    match tokio::time::timeout(Duration::from_secs(20), future).await {
        Ok(value) => value,
        Err(_) => panic!("{what} 超时(20s)"),
    }
}

#[tokio::test]
#[ignore = "需要真机:设置 SSH_TEST_HOST / SSH_TEST_USER / SSH_TEST_PASSWORD"]
async fn browses_the_remote_home_directory() {
    let Some(params) = params() else {
        eprintln!("跳过:没有设置 SSH_TEST_* 环境变量");
        return;
    };
    let fs = SshFs::new(params);

    // 第一次请求才建连接,所以这里同时覆盖了「懒连接 + 认证 + sftp 子系统」整条路。
    let home = with_timeout("取家目录", fs.home_dir()).await.expect("家目录");
    println!("家目录:{home}");
    assert!(home.starts_with('/'), "远端家目录应当是以 / 开头的绝对路径");

    let entries = with_timeout("列家目录", fs.list_dir(home.clone()))
        .await
        .expect("列家目录");
    println!("{home} 下 {} 项", entries.len());
    assert!(!entries.is_empty(), "家目录不该是空的");
    // 顺序约定与本地一致:目录在前、名字不区分大小写。
    let first_file = entries.iter().position(|entry| !entry.is_dir);
    if let Some(first_file) = first_file {
        assert!(
            entries[..first_file].iter().all(|entry| entry.is_dir),
            "目录应当排在文件之前"
        );
    }

    // 根目录一定存在且是目录;家目录本身也是目录。
    assert!(with_timeout("看 / 是不是目录", fs.is_dir("/"))
        .await
        .expect("stat /"));
    assert!(with_timeout("看家目录是不是目录", fs.is_dir(home.clone()))
        .await
        .expect("stat 家目录"));

    // 同一个句柄第二次请求复用连接(不该重连,也不该报错)。
    let again = with_timeout("再列一次 /etc", fs.list_dir("/etc"))
        .await
        .expect("/etc 应当可列");
    assert!(again.len() > 1, "/etc 下应当有多项");
}

#[tokio::test]
#[ignore = "需要真机:设置 SSH_TEST_HOST / SSH_TEST_USER / SSH_TEST_PASSWORD"]
async fn reports_a_missing_path_without_killing_the_connection() {
    let Some(params) = params() else {
        eprintln!("跳过:没有设置 SSH_TEST_* 环境变量");
        return;
    };
    let fs = SshFs::new(params);

    let missing = with_timeout("列一个不存在的目录", fs.list_dir("/definitely-not-here-1a2b3c"))
        .await;
    let message = missing.expect_err("不存在的路径应当报错").to_string();
    println!("错误:{message}");
    assert!(
        message.contains("远端没有这个路径"),
        "状态错误应当翻译成中文提示,实际:{message}"
    );

    // ⚠️ 关键:状态错误只是那一条路径的问题,**连接仍然可用**(见 `fs.rs` 里
    // 「只有传输层坏了才丢连接」那条规则)。
    let home = with_timeout("取家目录", fs.home_dir())
        .await
        .expect("状态错误之后连接应当仍然可用");
    println!("状态错误之后仍然可用,家目录:{home}");
}
