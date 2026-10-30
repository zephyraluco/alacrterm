//! Application-owned integration policy over the public SSH API.
use crate::local_server;
use futures::StreamExt as _;
use local_server::{BANNER, PASSWORD, PROMPT};
use ssh::{HostKeyDecision, HostKeyPolicy, SshEvent, SshParams, SshSession, SshSize};
use std::{path::PathBuf, sync::Arc, time::Duration};
fn temp_known_hosts(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("alacrterm-ssh-test-{name}-known_hosts"));
    let _ = std::fs::remove_file(&path);
    path
}

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

#[tokio::test]
#[ignore = "modifies startup files: requires an isolated OpenSSH account accepting integration env variables and SSH_INTEGRATION_TEST_PORT/KEY"]
async fn native_openssh_loads_persistent_integration_in_an_isolated_account() {
    let port: u16 = std::env::var("SSH_INTEGRATION_TEST_PORT")
        .unwrap()
        .parse()
        .unwrap();
    let key = PathBuf::from(std::env::var("SSH_INTEGRATION_TEST_KEY").unwrap());
    let user = std::env::var("SSH_INTEGRATION_TEST_USER").unwrap_or_else(|_| "agent".into());
    let known_hosts = temp_known_hosts("native-integration");
    let mut params = SshParams::new("127.0.0.1", port, user);
    params.key_files.push(key);
    params.host_key = HostKeyPolicy::AcceptNew;
    params.known_hosts = Some(known_hosts.clone());
    let (sender, mut events) = futures::channel::mpsc::unbounded();
    let session = SshSession::connect_with_setup(
        params,
        SshSize::cells(80, 24),
        sender,
        Some(Arc::new(super::remote::RemoteIntegration {
            history_enabled: true,
        })),
    )
    .unwrap();
    let (text, closed) = collect_until(&mut events, Duration::from_secs(15), None, |text, _| {
        text.contains("633;B") && text.contains("633;AlacrtermShell;posix")
    })
    .await;
    assert!(closed.is_none(), "native shell exited early");
    assert!(
        text.contains("633;AlacrtermShell;posix"),
        "integration was not loaded"
    );
    session.write(b"printf 'NATIVE_INTEGRATION_OK\\n'\n");
    let (output, _) = collect_until(&mut events, Duration::from_secs(5), None, |text, _| {
        text.contains("NATIVE_INTEGRATION_OK\r\n") && text.contains("633;C")
    })
    .await;
    assert!(output.contains("633;C"));
    assert!(output.contains("NATIVE_INTEGRATION_OK\r\n"));
    session.write(b"shopt -q login_shell && printf 'NATIVE_LOGIN_OK\\n'\n");
    let (login, _) = collect_until(&mut events, Duration::from_secs(5), None, |text, _| {
        text.contains("NATIVE_LOGIN_OK\r\n")
    })
    .await;
    assert!(
        login.contains("NATIVE_LOGIN_OK\r\n"),
        "integration replaced the login shell"
    );
    session.write(b"exit\n");
    let (_, closed) = collect_until(&mut events, Duration::from_secs(5), None, |_, closed| {
        closed.is_some()
    })
    .await;
    assert!(closed.is_some());
    let _ = std::fs::remove_file(known_hosts);
}

#[tokio::test]
#[cfg(unix)]
async fn persistent_startup_install_is_reused_and_never_echoed_or_recorded() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join(".bashrc"), "# USER_STARTUP\n").unwrap();
    let port = local_server::serve_with_integration_home(home.path().to_owned())
        .await
        .unwrap();
    let known_hosts = temp_known_hosts("persistent-startup");
    let mut params = SshParams::new("127.0.0.1", port, "tester").with_password(PASSWORD);
    params.host_key = HostKeyPolicy::AcceptNew;
    params.known_hosts = Some(known_hosts.clone());
    let mut first_rc = Vec::new();
    for attempt in 0..2 {
        let (sender, mut events) = futures::channel::mpsc::unbounded();
        let session = SshSession::connect_with_setup(
            params.clone(),
            SshSize::cells(80, 24),
            sender,
            Some(Arc::new(super::remote::RemoteIntegration {
                history_enabled: true,
            })),
        )
        .unwrap();
        let (text, _) = collect_until(&mut events, Duration::from_secs(10), None, |text, _| {
            text.contains("633;AlacrtermShell;posix") && text.ends_with(PROMPT)
        })
        .await;
        assert!(text.contains(BANNER));
        assert!(text.contains("633;AlacrtermShell;posix"));
        assert!(text.contains("PAM_MOTD: Welcome to Ubuntu\r\nVENDOR_WELCOME\r\n"));
        assert_eq!(text.matches("SSH_AUTH_NOTICE\r\n").count(), 1);
        assert!(text.find("SSH_AUTH_NOTICE").unwrap() < text.find("PAM_MOTD").unwrap());
        assert!(text.find("VENDOR_WELCOME").unwrap() < text.find(BANNER).unwrap());
        assert!(
            !text.contains("source ")
                && !text.contains("loader.sh")
                && !text.contains("command rm")
        );
        let manifest =
            std::fs::read_to_string(home.path().join(".local/share/alacrterm/shell/version"))
                .unwrap();
        assert_eq!(manifest.lines().next(), Some(super::VERSION));
        assert!(manifest.lines().any(|line| line == "bash"));
        let rc = std::fs::read(home.path().join(".bashrc")).unwrap();
        if attempt == 0 {
            first_rc = rc;
        } else {
            assert_eq!(rc, first_rc);
        }
        session.disconnect();
    }
    let (sender, mut events) = futures::channel::mpsc::unbounded();
    let session = SshSession::connect(params, SshSize::cells(80, 24), sender).unwrap();
    let (text, _) = collect_until(&mut events, Duration::from_secs(5), None, |text, _| {
        text.ends_with(PROMPT)
    })
    .await;
    assert!(text.contains(BANNER));
    assert!(!text.contains("633;AlacrtermShell"));
    session.disconnect();
    let _ = std::fs::remove_file(known_hosts);
}

#[tokio::test]
#[cfg(unix)]
async fn failed_install_revokes_activation_and_preserves_native_login() {
    let home = tempfile::tempdir().unwrap();
    let original = home.path().join("original-rc");
    std::fs::write(
        &original,
        "[ \"${LC_ALACRTERM_INTEGRATION:-0}\" != 1 ] || printf 'STALE_INTEGRATION_LOADED\\n'\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(&original, home.path().join(".bashrc")).unwrap();
    let port = local_server::serve_with_integration_home(home.path().to_owned())
        .await
        .unwrap();
    let known_hosts = temp_known_hosts("failed-single-exec");
    let mut params = SshParams::new("127.0.0.1", port, "tester").with_password(PASSWORD);
    params.host_key = HostKeyPolicy::AcceptNew;
    params.known_hosts = Some(known_hosts.clone());
    let (sender, mut events) = futures::channel::mpsc::unbounded();
    let session = SshSession::connect_with_setup(
        params,
        SshSize::cells(80, 24),
        sender,
        Some(Arc::new(super::remote::RemoteIntegration {
            history_enabled: true,
        })),
    )
    .unwrap();
    let (text, _) = collect_until(&mut events, Duration::from_secs(10), None, |text, _| {
        text.ends_with(PROMPT)
    })
    .await;
    assert!(text.contains("PAM_MOTD") && text.contains(BANNER), "{text}");
    assert!(!text.contains("STALE_INTEGRATION_LOADED"), "{text}");
    assert!(
        !home
            .path()
            .join(".local/share/alacrterm/shell/version")
            .exists()
    );
    session.disconnect();
    let _ = std::fs::remove_file(known_hosts);
}

#[tokio::test]
#[cfg(unix)]
async fn installed_remote_history_travels_over_ssh_and_honors_disabled_activation() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join(".bash_history"),
        "echo REMOTE_IMPORTED_OLD\n",
    )
    .unwrap();
    std::fs::write(
        home.path().join(".bashrc"),
        "HISTSIZE=2000\nHISTFILE=\"$HOME/.bash_history\"\nhistory -r\n",
    )
    .unwrap();
    let port = local_server::serve_with_integration_home(home.path().to_owned())
        .await
        .unwrap();
    let known_hosts = temp_known_hosts("remote-history");
    let mut params = SshParams::new("127.0.0.1", port, "tester").with_password(PASSWORD);
    params.host_key = HostKeyPolicy::AcceptNew;
    params.known_hosts = Some(known_hosts.clone());
    let (sender, mut events) = futures::channel::mpsc::unbounded();
    let session = SshSession::connect_with_setup(
        params.clone(),
        SshSize::cells(80, 24),
        sender,
        Some(Arc::new(super::remote::RemoteIntegration {
            history_enabled: true,
        })),
    )
    .unwrap();
    let (startup, _) = collect_until(&mut events, Duration::from_secs(10), None, |text, _| {
        text.ends_with(PROMPT)
    })
    .await;
    assert!(startup.contains("633;AlacrtermShell;posix"));
    session.disconnect();
    for activation in ["1", "1-no-history"] {
        let (sender, mut events) = futures::channel::mpsc::unbounded();
        let command = format!(
            "LC_ALACRTERM_INTEGRATION={activation} bash --noprofile --rcfile \"$HOME/.bashrc\" -ic '__alacrterm_history_sync; sleep .2'"
        );
        let session = SshSession::connect_exec(params.clone(), command, sender).unwrap();
        let (output, status) =
            collect_until(&mut events, Duration::from_secs(10), None, |_, status| {
                status.is_some()
            })
            .await;
        assert_eq!(status, Some(0), "{output}");
        let imported = output
            .split('\x07')
            .filter_map(|part| {
                let (_, payload) = part.rsplit_once("\x1b]")?;
                match super::protocol::decode(payload.as_bytes())? {
                    terminal::ShellMetadata::History(terminal::HistoryEvent::Entry {
                        command,
                        ..
                    }) => Some(command),
                    _ => None,
                }
            })
            .any(|command| command == "echo REMOTE_IMPORTED_OLD");
        assert_eq!(imported, activation == "1", "{output}");
        session.disconnect();
    }
    let _ = std::fs::remove_file(known_hosts);
}
