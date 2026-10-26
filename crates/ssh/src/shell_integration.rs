//! Optional session-scoped POSIX shell integration. Never edits user startup files.
use crate::{handler::ClientHandler, params::ShellIntegration};
use russh::{ChannelMsg, client};
use std::time::Duration;
const PROBE: &str = "printf '\\036ALACRTERM_SHELL:%s\\037' \"$SHELL\"";
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn parse_probe(bytes: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(bytes).ok()?;
    let value = text
        .split_once("\x1eALACRTERM_SHELL:")?
        .1
        .split_once('\x1f')?
        .0;
    let name = value.rsplit('/').next()?;
    (value.starts_with('/')
        && matches!(name, "bash" | "zsh")
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "/_.-".contains(ch)))
    .then_some(value)
}
pub(crate) async fn prepare(
    handle: &client::Handle<ClientHandler>,
    scripts: ShellIntegration,
) -> Option<String> {
    let mut probe = tokio::time::timeout(Duration::from_secs(2), handle.channel_open_session())
        .await
        .ok()?
        .ok()?;
    let result = tokio::time::timeout(Duration::from_secs(2), async {
        probe.exec(false, PROBE).await.ok()?;
        let mut bytes = Vec::new();
        while let Some(message) = probe.wait().await {
            match message {
                ChannelMsg::Data { data } if bytes.len() + data.len() <= 8192 => {
                    bytes.extend_from_slice(&data);
                    if let Some(shell) = parse_probe(&bytes) {
                        return Some(shell.to_owned());
                    }
                }
                ChannelMsg::Data { .. }
                | ChannelMsg::Eof
                | ChannelMsg::Close
                | ChannelMsg::Failure => break,
                _ => {}
            }
        }
        None
    })
    .await
    .ok()
    .flatten();
    let _ = probe.close().await;
    result.map(|shell| bootstrap(&shell, scripts))
}
fn write_file(body: &mut String, name: &str, value: &str) {
    // include_str! preserves Windows checkout line endings. POSIX shells require
    // LF even when the SSH client and its embedded scripts were built on Windows.
    let value = value.replace("\r\n", "\n");
    body.push_str(&format!("printf '%s' {} > \"$d/{name}\" 2>/dev/null || {{ rm -rf -- \"$d\"; trap - EXIT; umask \"$alacrterm_umask\"; exec \"$alacrterm_shell\" -l; }}\n", quote(&value)));
}
pub(crate) fn bootstrap(shell: &str, scripts: ShellIntegration) -> String {
    let mut body = format!(
        "alacrterm_shell={}\nalacrterm_umask=$(umask)\numask 077\nd=$(mktemp -d \"${{TMPDIR:-/tmp}}/alacrterm-shell.XXXXXXXX\" 2>/dev/null) || {{ umask \"$alacrterm_umask\"; exec \"$alacrterm_shell\" -l; }}\ntrap 'rm -rf -- \"$d\"' EXIT\ntrap 'exit 129' HUP\ntrap 'exit 130' INT\ntrap 'exit 143' TERM\nexport ALACRTERM_INTEGRATION_SCRIPT=\"$d/integration\"\n",
        quote(shell)
    );
    if shell.ends_with("/bash") {
        write_file(&mut body, "integration", scripts.bash);
        write_file(
            &mut body,
            "bashrc",
            r#"# Preserve login startup ordering without modifying any of those files.
[[ -r /etc/profile ]] && source /etc/profile
for alacrterm_profile in "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile"; do
    if [[ -r $alacrterm_profile ]]; then source "$alacrterm_profile"; break; fi
done
source "$ALACRTERM_INTEGRATION_SCRIPT"
"#,
        );
        body.push_str(&format!(
            "umask \"$alacrterm_umask\"\n{} --rcfile \"$d/bashrc\" -i\n",
            quote(shell)
        ));
    } else {
        write_file(&mut body, "integration", scripts.zsh);
        body.push_str("export ALACRTERM_ORIGINAL_ZDOTDIR=\"${ZDOTDIR:-$HOME}\"\nexport ALACRTERM_WRAPPER_DIR=\"$d\"\n");
        for name in [".zshenv", ".zprofile", ".zshrc", ".zlogin"] {
            let mut wrapper = format!(
                "ZDOTDIR=$ALACRTERM_ORIGINAL_ZDOTDIR\n[[ -r $ZDOTDIR/{name} ]] && source \"$ZDOTDIR/{name}\"\nALACRTERM_ORIGINAL_ZDOTDIR=${{ZDOTDIR:-$HOME}}\n"
            );
            if name == ".zshrc" {
                wrapper.push_str("source \"$ALACRTERM_INTEGRATION_SCRIPT\"\n");
            }
            if name != ".zlogin" {
                wrapper.push_str("ZDOTDIR=$ALACRTERM_WRAPPER_DIR\n");
            }
            write_file(&mut body, name, &wrapper);
        }
        body.push_str(&format!(
            "umask \"$alacrterm_umask\"\nZDOTDIR=\"$d\" {} -il\n",
            quote(shell)
        ));
    }
    body.push_str("alacrterm_exit=$?\nexit \"$alacrterm_exit\"\n");
    format!("exec sh -c {}", quote(&body))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bootstrap_normalizes_windows_scripts_and_wrappers_for_both_shells() {
        let lf = ShellIntegration { bash: "demo() {\n printf 'hello'\n}\n", zsh: "demo() {\n printf 'hello'\n}\n" };
        let crlf = ShellIntegration { bash: "demo() {\r\n printf 'hello'\r\n}\r\n", zsh: "demo() {\r\n printf 'hello'\r\n}\r\n" };
        for shell in ["/bin/bash", "/bin/zsh"] {
            let command = bootstrap(shell, crlf);
            assert_eq!(command, bootstrap(shell, lf));
            assert!(!command.contains('\r'));
        }
    }
    #[test]
    fn probe_accepts_only_framed_supported_posix_shells() {
        assert_eq!(
            parse_probe(b"noise\x1eALACRTERM_SHELL:/bin/bash\x1f"),
            Some("/bin/bash")
        );
        assert!(parse_probe(b"/bin/bash").is_none());
        assert!(parse_probe(b"\x1eALACRTERM_SHELL:/bin/fish\x1f").is_none());
        assert!(parse_probe(b"\x1eALACRTERM_SHELL:/bin/evil;bash\x1f").is_none());
    }
    #[test]
    #[cfg(unix)]
    fn bash_bootstrap_loads_profiles_once_and_removes_session_files() {
        use std::{
            io::Write as _,
            process::{Command, Stdio},
        };
        let root = std::env::temp_dir().join(format!("alacrterm-bootstrap-{}", std::process::id()));
        std::fs::create_dir_all(root.join("temporary")).unwrap();
        std::fs::write(
            root.join(".bash_profile"),
            "printf 'USER_PROFILE\\n'\nPS1='bootstrap$ '\n",
        )
        .unwrap();
        let scripts = ShellIntegration {
            bash: Box::leak(include_str!("../../terminal/shell-integration/bash.sh")
                .replace("\r\n", "\n").replace('\n', "\r\n").into_boxed_str()),
            zsh: "",
        };
        let mut child = Command::new("sh")
            .args(["-c", &bootstrap("/bin/bash", scripts)])
            .env("HOME", &root)
            .env("TMPDIR", root.join("temporary"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"echo READY\nexit 7\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        let mut bytes = output.stdout;
        bytes.extend(output.stderr);
        let text = String::from_utf8_lossy(&bytes);
        assert_eq!(output.status.code(), Some(7));
        assert_eq!(text.matches("USER_PROFILE").count(), 1);
        assert!(text.contains("633;AlacrtermShell;posix"));
        assert_eq!(
            std::fs::read_dir(root.join("temporary")).unwrap().count(),
            0
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[cfg(unix)]
    fn zsh_bootstrap_preserves_login_stages_and_cleans_session_files() {
        use std::{
            io::Write as _,
            process::{Command, Stdio},
        };
        if !std::path::Path::new("/usr/bin/zsh").exists() {
            return;
        }
        let root =
            std::env::temp_dir().join(format!("alacrterm-bootstrap-zsh-{}", std::process::id()));
        std::fs::create_dir_all(root.join("temporary")).unwrap();
        for (file, marker) in [
            (".zshenv", "ENV_STAGE"),
            (".zprofile", "PROFILE_STAGE"),
            (".zshrc", "RC_STAGE"),
            (".zlogin", "LOGIN_STAGE"),
        ] {
            std::fs::write(root.join(file), format!("printf '{marker}\\n'\n")).unwrap();
        }
        let scripts = ShellIntegration {
            bash: "",
            zsh: Box::leak(include_str!("../../terminal/shell-integration/zsh.zsh")
                .replace("\r\n", "\n").replace('\n', "\r\n").into_boxed_str()),
        };
        let mut child = Command::new("sh")
            .args(["-c", &bootstrap("/usr/bin/zsh", scripts)])
            .env("HOME", &root)
            .env("ZDOTDIR", &root)
            .env("TMPDIR", root.join("temporary"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"exit 9\n").unwrap();
        let output = child.wait_with_output().unwrap();
        let mut bytes = output.stdout;
        bytes.extend(output.stderr);
        let text = String::from_utf8_lossy(&bytes);
        assert_eq!(output.status.code(), Some(9));
        for marker in ["ENV_STAGE", "PROFILE_STAGE", "RC_STAGE", "LOGIN_STAGE"] {
            assert_eq!(text.matches(marker).count(), 1);
        }
        assert!(text.contains("633;AlacrtermShell;posix"));
        assert_eq!(
            std::fs::read_dir(root.join("temporary")).unwrap().count(),
            0
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
