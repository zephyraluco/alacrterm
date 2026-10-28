use crate::local_server;
use futures::StreamExt as _;
use local_server::{EXIT_CODE, PASSWORD};
use ssh::{HostKeyDecision, HostKeyPolicy, SshEvent, SshParams, SshSession, SshSize};
use std::{path::PathBuf, time::Duration};
fn options() -> ssh::SessionOptions {
    ssh::SessionOptions {
        x11: Some(std::sync::Arc::new(super::WayxTarget)),
        ..Default::default()
    }
}
fn temp_known_hosts(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("alacrterm-ssh-test-{name}-known_hosts"));
    let _ = std::fs::remove_file(&path);
    path
}

/// 连上本地测试服务端并返回会话 + 事件流。
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

/// Concurrent SSH sessions share WayX, and closing one must not disrupt the other.
#[tokio::test]
#[ignore = "requires a host display and bundled WayX runtime"]
async fn shared_x11_server_survives_disconnecting_another_session() {
    struct Cleanup;
    impl Drop for Cleanup {
        fn drop(&mut self) {
            platform::shutdown_wayx();
        }
    }
    let _cleanup = Cleanup;
    platform::start_wayx();
    tokio::time::timeout(Duration::from_secs(20), async {
        while platform::wayx_status() != platform::WayxStatus::Running {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    let port = local_server::serve_with_compression(vec![russh::compression::ZLIB_LEGACY].into())
        .await
        .expect("local SSH server with delayed compression");
    let known_hosts = temp_known_hosts("x11-forwarding");
    let mut params = SshParams::new("127.0.0.1", port, "tester").with_password(PASSWORD);
    params.host_key = HostKeyPolicy::AcceptNew;
    params.known_hosts = Some(known_hosts.clone());
    let (tx, mut events) = futures::channel::mpsc::unbounded();
    let first =
        SshSession::connect_with_options(params.clone(), SshSize::cells(80, 24), tx, options())
            .unwrap();
    let (tx, mut other_events) = futures::channel::mpsc::unbounded();
    let second =
        SshSession::connect_with_options(params, SshSize::cells(80, 24), tx, options()).unwrap();
    let text = collect_until(&mut events, Duration::from_secs(25), None, |text, _| {
        text.contains("X11:OK") || text.contains("X11:FAILED")
    })
    .await;
    assert!(text.0.contains("X11:OK"), "{text:?}");
    let text = collect_until(
        &mut other_events,
        Duration::from_secs(25),
        None,
        |text, _| text.contains("X11:OK") || text.contains("X11:FAILED"),
    )
    .await;
    assert!(text.0.contains("X11:OK"), "{text:?}");
    first.write(b"exit\r");
    let (_, exit) = collect_until(&mut events, Duration::from_secs(5), None, |_, _| false).await;
    assert_eq!(exit, Some(EXIT_CODE));
    drop(first);
    second.write(b"x11\r");
    let text = collect_until(
        &mut other_events,
        Duration::from_secs(5),
        None,
        |text, _| text.contains("X11:OK") || text.contains("X11:FAILED"),
    )
    .await;
    assert!(text.0.contains("X11:OK"), "{text:?}");
    platform::stop_wayx();
    tokio::time::timeout(Duration::from_secs(5), async {
        while platform::wayx_status() != platform::WayxStatus::Stopped {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    second.write(b"still-connected\r");
    let (text, _) = collect_until(
        &mut other_events,
        Duration::from_secs(5),
        None,
        |text, _| text.contains("REPLY:still-connected"),
    )
    .await;
    assert!(text.contains("REPLY:still-connected"));
    platform::start_wayx();
    tokio::time::timeout(Duration::from_secs(20), async {
        while platform::wayx_status() != platform::WayxStatus::Running {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    second.write(b"x11\r");
    let (text, _) = collect_until(
        &mut other_events,
        Duration::from_secs(5),
        None,
        |text, _| text.contains("X11:OK") || text.contains("X11:FAILED"),
    )
    .await;
    assert!(text.contains("X11:OK"), "{text}");
    second.write(b"exit\r");
    let (_, exit) = collect_until(&mut other_events, Duration::from_secs(5), None, |_, _| {
        false
    })
    .await;
    assert_eq!(exit, Some(EXIT_CODE));
    drop(second);
    let _ = std::fs::remove_file(known_hosts);
}

#[tokio::test]
async fn rejects_x11_silently_and_preserves_interactive_shell() {
    let port = local_server::serve().await.unwrap();
    let known_hosts = temp_known_hosts("x11-refusal");
    let mut params = SshParams::new("127.0.0.1", port, "no-x11").with_password(PASSWORD);
    params.host_key = HostKeyPolicy::AcceptNew;
    params.known_hosts = Some(known_hosts.clone());
    let (tx, mut events) = futures::channel::mpsc::unbounded();
    let session =
        SshSession::connect_with_options(params, SshSize::cells(80, 24), tx, options()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = events.next().await {
            match event {
                SshEvent::Status(text) => assert!(!text.contains("X11") && !text.contains("WayX")),
                SshEvent::Connected => break,
                SshEvent::Closed { reason, .. } => panic!("shell closed: {reason:?}"),
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    session.write(b"fallback-ok\r");
    let (text, _) = collect_until(&mut events, Duration::from_secs(5), None, |text, _| {
        text.contains("REPLY:fallback-ok")
    })
    .await;
    assert!(text.contains("REPLY:fallback-ok"));
    assert_eq!(platform::wayx_status(), platform::WayxStatus::Stopped);
    session.disconnect();
    let _ = std::fs::remove_file(known_hosts);
}
