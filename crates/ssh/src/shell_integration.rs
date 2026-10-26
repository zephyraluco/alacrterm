//! Install optional startup integration before requesting the native login shell.
use crate::{
    auth,
    handler::ClientHandler,
    params::{HostKeyPolicy, ShellIntegration, SshParams},
    session::EventSink,
};
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
    tokio::time::timeout(Duration::from_secs(5), async {
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
fn installation(kind: &str, scripts: ShellIntegration, zdotdir: &str) -> String {
    let lf = |text: &str| text.replace("\r\n", "\n");
    format!(
        "sh -c {} -- {} {} {} {} {}",
        quote(&lf(include_str!("../../../scripts/shell_integration/install.sh"))),
        quote(kind),
        quote(&lf(scripts.bash)),
        quote(&lf(scripts.zsh)),
        quote(&lf(include_str!("../../../scripts/shell_integration/loader.sh"))),
        quote(zdotdir)
    )
}

/// Use a separate transport: OpenSSH consumes connection-scoped PAM login
/// messages on the first exec, even when that command's output is discarded.
/// The terminal transport must execute request_shell first.
pub(crate) async fn install(
    params: &SshParams,
    scripts: ShellIntegration,
    events: &EventSink,
) -> bool {
    tokio::time::timeout(Duration::from_secs(20), async {
        // The interactive connection has already verified and recorded this key.
        // Never ask again or accept a different unknown key on the helper.
        let handler = ClientHandler::new(
            params.user.clone(),
            params.host.clone(),
            params.port,
            HostKeyPolicy::Strict,
            params.known_hosts.clone(),
            None,
        );
        let config = std::sync::Arc::new(client::Config {
            preferred: crate::compression::preferred(),
            nodelay: true,
            ..Default::default()
        });
        let Ok(mut helper) =
            client::connect(config, (params.host.as_str(), params.port), handler).await
        else {
            return false;
        };
        let installed = auth::authenticate(&mut helper, params, events)
            .await
            .is_ok()
            && install_on_handle(&helper, scripts).await;
        let _ = helper
            .disconnect(russh::Disconnect::ByApplication, "", "")
            .await;
        installed
    })
    .await
    .unwrap_or(false)
}

async fn install_on_handle(
    handle: &client::Handle<ClientHandler>,
    scripts: ShellIntegration,
) -> bool {
    let Some(probe) = exec(handle, "printf '\\036ALACRTERM_SHELL:%s\\037\\036ALACRTERM_ZDOTDIR:%s\\037' \"$SHELL\" \"${ZDOTDIR:-$HOME}\"".into()).await else { return false; };
    let Some(shell) = framed(&probe, "ALACRTERM_SHELL") else {
        return false;
    };
    let Some(kind @ ("bash" | "zsh")) = shell.rsplit('/').next() else {
        return false;
    };
    let Some(result) = exec(
        handle,
        installation(
            kind,
            scripts,
            framed(&probe, "ALACRTERM_ZDOTDIR").unwrap_or(""),
        ),
    )
    .await
    else {
        return false;
    };
    framed(&result, "ALACRTERM_INSTALLED") == Some("1")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(unix)]
    fn managed_symlinks_are_not_replaced_and_custom_zdotdir_is_respected() {
        use std::os::unix::fs::symlink;
        let home = tempfile::tempdir().unwrap();
        let original = home.path().join("original-rc");
        std::fs::write(&original, "# KEEP\n").unwrap();
        symlink(&original, home.path().join(".bashrc")).unwrap();
        assert!(
            !run_install(home.path(), "bash", "# script\n")
                .status
                .success()
        );
        assert_eq!(std::fs::read_to_string(original).unwrap(), "# KEEP\n");
        let zdir = home.path().join("zsh-config");
        std::fs::create_dir(&zdir).unwrap();
        let output = std::process::Command::new("sh")
            .args([
                "-c",
                &installation(
                    "zsh",
                    ShellIntegration {
                        bash: "",
                        zsh: "printf 'ZSH_READY\\n'\n",
                    },
                    zdir.to_str().unwrap(),
                ),
            ])
            .env("HOME", home.path())
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(zdir.join(".zshrc").exists());
        assert!(!home.path().join(".zshrc").exists());
        if std::path::Path::new("/usr/bin/zsh").exists() {
            let output = std::process::Command::new("/usr/bin/zsh")
                .args(["-d", "-ic", "history; :"])
                .env("HOME", home.path())
                .env("ZDOTDIR", &zdir)
                .env("LC_ALACRTERM_INTEGRATION", "1")
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(String::from_utf8_lossy(&output.stdout), "ZSH_READY\n");
        }
    }
    #[test]
    fn embedded_scripts_are_normalized_before_installation() {
        let lf = ShellIntegration {
            bash: "one\ntwo\n",
            zsh: "three\n",
        };
        let crlf = ShellIntegration {
            bash: "one\r\ntwo\r\n",
            zsh: "three\r\n",
        };
        assert_eq!(installation("bash", lf, ""), installation("bash", crlf, ""));
    }
    #[cfg(unix)]
    fn run_install(
        home: &std::path::Path,
        kind: &str,
        script: &'static str,
    ) -> std::process::Output {
        std::process::Command::new("sh")
            .args([
                "-c",
                &installation(
                    kind,
                    ShellIntegration {
                        bash: script,
                        zsh: script,
                    },
                    "",
                ),
            ])
            .env("HOME", home)
            .env("ZDOTDIR", home)
            .output()
            .unwrap()
    }
    #[test]
    #[cfg(unix)]
    fn installation_is_idempotent_updates_scripts_and_preserves_user_config() {
        let home = tempfile::tempdir().unwrap();
        let rc = home.path().join(".bashrc");
        std::fs::write(&rc, "# USER_CONFIG\n").unwrap();
        assert!(
            run_install(home.path(), "bash", "# first\r\n")
                .status
                .success()
        );
        let before = std::fs::read(&rc).unwrap();
        let script = home.path().join(".local/share/alacrterm/shell/bash.sh");
        let modified = std::fs::metadata(&script).unwrap().modified().unwrap();
        assert!(
            run_install(home.path(), "bash", "# first\n")
                .status
                .success()
        );
        assert_eq!(
            std::fs::metadata(&script).unwrap().modified().unwrap(),
            modified
        );
        assert!(
            run_install(home.path(), "bash", "# updated\n")
                .status
                .success()
        );
        assert_eq!(std::fs::read(&rc).unwrap(), before);
        assert_eq!(std::fs::read_to_string(script).unwrap(), "# updated\n");
        assert_eq!(
            String::from_utf8(before)
                .unwrap()
                .matches("# >>> alacrterm")
                .count(),
            1
        );
        std::fs::write(&rc, "# ENTRY_REMOVED_BY_USER\n").unwrap();
        assert!(
            !run_install(home.path(), "bash", "# next\n")
                .status
                .success()
        );
        assert_eq!(
            std::fs::read_to_string(rc).unwrap(),
            "# ENTRY_REMOVED_BY_USER\n"
        );
    }
    #[test]
    #[cfg(unix)]
    fn startup_loads_before_first_prompt_without_creating_history_or_affecting_other_clients() {
        let home = tempfile::tempdir().unwrap();
        assert!(
            run_install(home.path(), "bash", "printf 'INTEGRATION_LOADED\\n'\n")
                .status
                .success()
        );
        let launch = |enabled: bool| {
            let mut command = std::process::Command::new("bash");
            command
                .args([
                    "--noprofile",
                    "--rcfile",
                    home.path().join(".bashrc").to_str().unwrap(),
                    "-ic",
                    "history; exit",
                ])
                .env("HOME", home.path())
                .env_remove("ALACRTERM_REMOTE_INTEGRATION")
                .env_remove("LC_ALACRTERM_INTEGRATION");
            if enabled {
                command.env("LC_ALACRTERM_INTEGRATION", "1");
            }
            command.output().unwrap()
        };
        let output = launch(true);
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success());
        assert_eq!(text, "INTEGRATION_LOADED\n");
        assert!(!String::from_utf8_lossy(&launch(false).stdout).contains("INTEGRATION_LOADED"));
    }
    #[test]
    #[cfg(unix)]
    fn zsh_installs_in_zdotdir_and_retains_existing_content() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(".zshrc"), "# EXISTING\n").unwrap();
        assert!(
            run_install(home.path(), "zsh", "# script\n")
                .status
                .success()
        );
        let rc = std::fs::read_to_string(home.path().join(".zshrc")).unwrap();
        assert!(rc.starts_with("# EXISTING\n"));
        assert!(
            run_install(home.path(), "zsh", "# script\n")
                .status
                .success()
        );
        assert_eq!(
            std::fs::read_to_string(home.path().join(".zshrc")).unwrap(),
            rc
        );
    }
}
