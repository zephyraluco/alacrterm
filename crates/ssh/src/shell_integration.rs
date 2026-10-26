//! Source optional integration in the existing login shell; never replace that shell.
use crate::{handler::ClientHandler, params::ShellIntegration};
use russh::{ChannelMsg, client};
use std::time::Duration;
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn framed<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    text.split_once(&format!("\x1e{tag}:"))?
        .1
        .split_once('\x1f')
        .map(|pair| pair.0)
}
async fn exec(handle: &client::Handle<ClientHandler>, command: String) -> Option<String> {
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut channel = handle.channel_open_session().await.ok()?;
        channel.exec(true, command).await.ok()?;
        let mut output = Vec::new();
        let mut success = false;
        while let Some(message) = channel.wait().await {
            match message {
                ChannelMsg::Data { data } if output.len() + data.len() <= 8192 => {
                    output.extend_from_slice(&data)
                }
                ChannelMsg::ExitStatus { exit_status } => success = exit_status == 0,
                ChannelMsg::Close => break,
                ChannelMsg::Data { .. } | ChannelMsg::Failure => return None,
                _ => {}
            }
        }
        success.then(|| String::from_utf8(output).ok()).flatten()
    })
    .await
    .ok()
    .flatten()
}
fn setup(script: &str) -> String {
    let script = script.replace("\r\n", "\n");
    format!(
        "umask 077; d=$(mktemp -d \"${{TMPDIR:-/tmp}}/alacrterm-shell.XXXXXXXX\") || exit 1; printf '%s' {} > \"$d/integration\" || {{ rm -rf -- \"$d\"; exit 1; }}; printf '\\036ALACRTERM_SCRIPT:%s\\037' \"$d/integration\"",
        quote(&script)
    )
}
pub(crate) async fn prepare(
    handle: &client::Handle<ClientHandler>,
    scripts: ShellIntegration,
) -> Option<String> {
    let probe = exec(
        handle,
        "printf '\\036ALACRTERM_SHELL:%s\\037' \"$SHELL\"".into(),
    )
    .await?;
    let shell = framed(&probe, "ALACRTERM_SHELL")?;
    let script = match shell.rsplit('/').next()? {
        "bash" => scripts.bash,
        "zsh" => scripts.zsh,
        _ => return None,
    };
    let result = exec(handle, setup(script)).await?;
    let path = framed(&result, "ALACRTERM_SCRIPT")?;
    let directory = std::path::Path::new(path).parent()?.to_str()?;
    if !path.starts_with('/')
        || !path.ends_with("/integration")
        || !directory
            .rsplit('/')
            .next()?
            .starts_with("alacrterm-shell.")
    {
        return None;
    }
    // A leading space avoids history in shells configured with ignorespace; no
    // history settings or user startup files are modified to hide this command.
    Some(format!(
        " . {}; command rm -rf -- {}\n",
        quote(path),
        quote(directory)
    ))
}

/// Conservative readiness hint, not a general shell prompt parser. Unrecognised
/// prompts (including startup questions) time out to an ordinary login session.
#[derive(Default)]
pub(crate) struct PromptGate {
    line: Vec<u8>,
    escape: u8,
}
impl PromptGate {
    pub fn observe(&mut self, bytes: &[u8]) -> bool {
        for &byte in bytes {
            match self.escape {
                1 => {
                    self.escape = match byte {
                        b'[' => 2,
                        b']' => 3,
                        _ => 0,
                    }
                }
                2 => {
                    if (0x40..=0x7e).contains(&byte) {
                        self.escape = 0;
                    }
                }
                3 => match byte {
                    7 => self.escape = 0,
                    27 => self.escape = 4,
                    _ => {}
                },
                4 => self.escape = if byte == b'\\' { 0 } else { 3 },
                _ => match byte {
                    27 => self.escape = 1,
                    b'\r' | b'\n' => self.line.clear(),
                    0x20..=0x7e if self.line.len() < 512 => self.line.push(byte),
                    _ => {
                        self.line.clear();
                        return false;
                    }
                },
            }
        }
        if self.escape != 0 {
            return false;
        }
        let Ok(line) = std::str::from_utf8(&self.line) else {
            return false;
        };
        let Some(prefix) = line
            .strip_suffix("$ ")
            .or_else(|| line.strip_suffix("# "))
            .or_else(|| line.strip_suffix("% "))
        else {
            return false;
        };
        if let Some((identity, _path)) = prefix.split_once(':') {
            if let Some((user, host)) = identity.split_once('@') {
                return !user.is_empty()
                    && !host.is_empty()
                    && identity
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"@._-".contains(&b));
            }
        }
        prefix.starts_with("bash-") && prefix[5..].bytes().all(|b| b.is_ascii_digit() || b == b'.')
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readiness_preserves_split_colored_prompt_and_rejects_banner_and_questions() {
        let mut gate = PromptGate::default();
        assert!(!gate.observe(b"Welcome to Ubuntu\r\n############ \r\nPassword: "));
        assert!(!gate.observe(b"\r\n\x1b[32mroot@robot:~"));
        assert!(gate.observe(b"# \x1b[0m"));
        assert!(!PromptGate::default().observe(b"Continue? [y/N] "));
    }
    #[test]
    fn script_preparation_normalizes_crlf_and_does_not_launch_another_shell() {
        assert_eq!(setup("demo() {\r\n :\r\n}\r\n"), setup("demo() {\n :\n}\n"));
        assert!(!setup("test").contains("--rcfile"));
    }
    #[test]
    #[cfg(unix)]
    fn prepared_script_is_sourced_in_the_current_interactive_shell_and_removed() {
        use std::process::Command;
        let prepared = Command::new("sh")
            .args([
                "-c",
                &setup("[[ $- == *i* ]] || return\r\nprintf 'INTEGRATION_READY\\n'\r\n"),
            ])
            .output()
            .unwrap();
        assert!(prepared.status.success());
        let text = String::from_utf8(prepared.stdout).unwrap();
        let path = framed(&text, "ALACRTERM_SCRIPT").unwrap();
        let directory = std::path::Path::new(path).parent().unwrap();
        let output = Command::new("bash")
            .args([
                "--noprofile",
                "--norc",
                "-ic",
                &format!(
                    "before=$$; . {}; command rm -rf -- {}; test \"$before\" = \"$$\"",
                    quote(path),
                    quote(directory.to_str().unwrap())
                ),
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("INTEGRATION_READY"));
        assert!(!directory.exists());
    }
}
