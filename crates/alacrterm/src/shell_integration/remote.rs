//! Application-owned remote installation policy.
use super::Scripts as ShellIntegration;
use ssh::{SessionContext, SessionSetup, SshCommandConnection};
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
async fn exec(handle: &SshCommandConnection, command: String) -> Option<String> {
    let output = handle
        .exec(&command, Duration::from_secs(5), 8192)
        .await
        .ok()?;
    (output.exit_status == Some(0))
        .then(|| String::from_utf8(output.stdout).ok())
        .flatten()
}
fn installation(version: &str, scripts: ShellIntegration, shells_file: &str) -> String {
    let lf = |text: &str| text.replace("\r\n", "\n");
    format!(
        "sh -c {} -- {} {} {} {} \"$SHELL\" \"${{ZDOTDIR:-$HOME}}\" {}",
        quote(&lf(include_str!(
            "../../../../scripts/shell_integration/install.sh"
        ))),
        quote(version),
        quote(&lf(scripts.bash)),
        quote(&lf(scripts.zsh)),
        quote(&lf(include_str!(
            "../../../../scripts/shell_integration/loader.sh"
        ))),
        quote(shells_file)
    )
}

pub(super) struct RemoteIntegration;
impl SessionSetup for RemoteIntegration {
    fn prepare<'a>(
        &'a self,
        mut session: SessionContext<'a>,
    ) -> futures::future::BoxFuture<'a, ()> {
        Box::pin(async move {
            let mut activated = None;
            for variable in ["ALACRTERM_REMOTE_INTEGRATION", "LC_ALACRTERM_INTEGRATION"] {
                match session.set_env(variable, "1", Duration::from_secs(2)).await {
                    Ok(true) => {
                        activated = Some(variable);
                        break;
                    }
                    Ok(false) => {}
                    Err(error) => {
                        log::debug!("Integration activation unavailable: {error:#}");
                        return;
                    }
                }
            }
            let Some(variable) = activated else {
                return;
            };
            let installed = tokio::time::timeout(Duration::from_secs(20), async {
                let Ok(helper) = session.connect_auxiliary().await else {
                    return false;
                };
                let installed = install_on_handle(&helper, super::scripts()).await;
                let _ = helper.disconnect().await;
                installed
            })
            .await
            .unwrap_or(false);
            if !installed {
                // A partially installed/old script must not activate after fallback.
                let _ = session.set_env(variable, "0", Duration::from_secs(2)).await;
                log::debug!("Persistent shell integration unavailable; continuing native login");
            }
        })
    }
}

async fn install_on_handle(handle: &SshCommandConnection, scripts: ShellIntegration) -> bool {
    let Some(result) = exec(handle, installation(super::VERSION, scripts, "/etc/shells")).await
    else {
        return false;
    };
    framed(&result, "ALACRTERM_INSTALLED") == Some("1")
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(
            installation("1.0.0", lf, "/etc/shells"),
            installation("1.0.0", crlf, "/etc/shells")
        );
    }
    #[cfg(unix)]
    mod unix {
        use super::*;
        use std::{
            path::{Path, PathBuf},
            process::{Command, Output},
        };
        fn install(home: &Path, shells: &str, version: &str, script: &'static str) -> Output {
            let list = home.join("shells-list");
            std::fs::write(&list, shells).unwrap();
            Command::new("sh")
                .args([
                    "-c",
                    &installation(
                        version,
                        ShellIntegration {
                            bash: script,
                            zsh: script,
                        },
                        list.to_str().unwrap(),
                    ),
                ])
                .env("HOME", home)
                .env("SHELL", "/bin/bash")
                .env("ZDOTDIR", home)
                .output()
                .unwrap()
        }
        fn ready(output: Output) {
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                framed(
                    &String::from_utf8_lossy(&output.stdout),
                    "ALACRTERM_INSTALLED"
                ),
                Some("1")
            );
        }
        fn directory(home: &Path) -> PathBuf {
            home.join(".local/share/alacrterm/shell")
        }
        #[test]
        fn matching_manifest_is_read_only_even_with_missing_entries_and_a_lock() {
            let home = tempfile::tempdir().unwrap();
            ready(install(home.path(), "/bin/bash\n", "1.0.0", "# original\n"));
            let dir = directory(home.path());
            let before = std::fs::metadata(dir.join("version"))
                .unwrap()
                .modified()
                .unwrap();
            std::fs::remove_file(home.path().join(".bashrc")).unwrap();
            std::fs::remove_file(dir.join("bash.sh")).unwrap();
            std::fs::create_dir(dir.join(".install-lock")).unwrap();
            ready(install(
                home.path(),
                "# comment\n/usr/bin/bash\n/bin/bash # duplicate\n/bin/sh\n/no/such/zsh\n",
                "1.0.0",
                "# ignored\n",
            ));
            assert!(!home.path().join(".bashrc").exists());
            assert!(!dir.join("bash.sh").exists());
            assert!(dir.join(".install-lock").is_dir());
            assert_eq!(
                std::fs::metadata(dir.join("version"))
                    .unwrap()
                    .modified()
                    .unwrap(),
                before
            );
        }
        #[test]
        fn upgrades_preserve_user_content_and_commit_version_last() {
            let home = tempfile::tempdir().unwrap();
            let rc = home.path().join(".bashrc");
            std::fs::write(&rc, "# USER_CONFIG\n").unwrap();
            ready(install(home.path(), "/bin/bash\n", "1.0.0", "# first\r\n"));
            let before = std::fs::read(&rc).unwrap();
            let dir = directory(home.path());
            ready(install(home.path(), "/bin/bash\n", "1.0.1", "# updated\n"));
            assert_eq!(std::fs::read(&rc).unwrap(), before);
            assert_eq!(
                std::fs::read_to_string(dir.join("bash.sh")).unwrap(),
                "# updated\n"
            );
            assert_eq!(
                std::fs::read_to_string(dir.join("version")).unwrap(),
                "1.0.1\nbash\n"
            );
            std::fs::write(&rc, "# >>> alacrterm shell integration >>>\n# edited\n").unwrap();
            assert!(
                !install(home.path(), "/bin/bash\n", "1.0.2", "# next\n")
                    .status
                    .success()
            );
            assert_eq!(
                std::fs::read_to_string(dir.join("version")).unwrap(),
                "1.0.1\nbash\n"
            );
            assert_eq!(
                std::fs::read_to_string(dir.join("bash.sh")).unwrap(),
                "# updated\n"
            );
        }
        #[test]
        fn installs_all_supported_shells_and_resolves_unexported_zdotdir() {
            let Some(zsh) = ["/bin/zsh", "/usr/bin/zsh"]
                .into_iter()
                .find(|path| Path::new(path).is_file())
            else {
                return;
            };
            let shells = format!("/bin/bash\n{zsh}\n{zsh} # duplicate\n/bin/sh\n");
            let home = tempfile::tempdir().unwrap();
            let zdir = home.path().join("zsh-config");
            std::fs::write(
                home.path().join(".zshenv"),
                format!("ZDOTDIR='{}'\n", zdir.display()),
            )
            .unwrap();
            ready(install(home.path(), "/bin/bash\n", "1.0.0", "# first\n"));
            ready(install(home.path(), &shells, "1.0.0", "# updated\n"));
            let dir = directory(home.path());
            assert_eq!(
                std::fs::read_to_string(dir.join("version")).unwrap(),
                "1.0.0\nbash\nzsh\n"
            );
            assert!(zdir.join(".zshrc").exists());
            assert!(!home.path().join(".zshrc").exists());
            // Order and duplicates do not force a write or inspect entries.
            std::fs::write(dir.join("version"), "1.0.0\nzsh\nbash\nzsh\n").unwrap();
            std::fs::remove_file(zdir.join(".zshrc")).unwrap();
            ready(install(home.path(), &shells, "1.0.0", "# ignored\n"));
            assert!(!zdir.join(".zshrc").exists());
            assert_eq!(
                std::fs::read_to_string(dir.join("version")).unwrap(),
                "1.0.0\nzsh\nbash\nzsh\n"
            );
            ready(install(
                home.path(),
                "/bin/bash\n",
                "1.0.0",
                "# bash only\n",
            ));
            assert_eq!(
                std::fs::read_to_string(dir.join("version")).unwrap(),
                "1.0.0\nbash\n"
            );
        }
        #[test]
        fn migrates_legacy_markers_without_duplicate_startup_blocks() {
            let home = tempfile::tempdir().unwrap();
            ready(install(home.path(), "/bin/bash\n", "1.0.0", "# script\n"));
            let dir = directory(home.path());
            let rc = std::fs::read(home.path().join(".bashrc")).unwrap();
            std::fs::remove_file(dir.join("version")).unwrap();
            std::fs::write(dir.join("installed-bash"), "1").unwrap();
            ready(install(home.path(), "/bin/bash\n", "1.0.0", "# script\n"));
            assert_eq!(std::fs::read(home.path().join(".bashrc")).unwrap(), rc);
            assert!(!dir.join("installed-bash").exists());
            // Removing the manifest explicitly requests repair.
            std::fs::remove_file(dir.join("version")).unwrap();
            std::fs::remove_file(home.path().join(".bashrc")).unwrap();
            ready(install(home.path(), "/bin/bash\n", "1.0.0", "# script\n"));
            assert!(home.path().join(".bashrc").exists());
        }
        #[test]
        fn symlinks_are_preserved_and_failed_install_does_not_commit_manifest() {
            let home = tempfile::tempdir().unwrap();
            let original = home.path().join("original-rc");
            std::fs::write(&original, "# KEEP\n").unwrap();
            std::os::unix::fs::symlink(&original, home.path().join(".bashrc")).unwrap();
            assert!(
                !install(home.path(), "/bin/bash\n", "1.0.0", "# script\n")
                    .status
                    .success()
            );
            assert_eq!(std::fs::read_to_string(original).unwrap(), "# KEEP\n");
            assert!(!directory(home.path()).join("version").exists());
        }
        #[test]
        fn no_supported_shell_leaves_home_unchanged() {
            let home = tempfile::tempdir().unwrap();
            let output = install(
                home.path(),
                "/bin/sh\n/no/such/bash\n",
                "1.0.0",
                "# script\n",
            );
            assert!(output.status.success());
            assert_eq!(
                framed(
                    &String::from_utf8_lossy(&output.stdout),
                    "ALACRTERM_INSTALLED"
                ),
                Some("unsupported")
            );
            assert!(!home.path().join(".local").exists());
        }
        #[test]
        fn missing_shell_list_falls_back_and_invalid_manifest_is_replaced() {
            let home = tempfile::tempdir().unwrap();
            let command = installation(
                "1.0.0",
                ShellIntegration {
                    bash: "# bash\n",
                    zsh: "# zsh\n",
                },
                home.path().join("absent-shell-list").to_str().unwrap(),
            );
            let run = || {
                Command::new("sh")
                    .args(["-c", &command])
                    .env("HOME", home.path())
                    .env("SHELL", "/bin/bash")
                    .output()
                    .unwrap()
            };
            ready(run());
            let manifest = directory(home.path()).join("version");
            assert_eq!(std::fs::read_to_string(&manifest).unwrap(), "1.0.0\nbash\n");
            std::fs::write(&manifest, "invalid\nunknown\n").unwrap();
            ready(run());
            assert_eq!(std::fs::read_to_string(manifest).unwrap(), "1.0.0\nbash\n");
        }
        #[test]
        fn default_zsh_preserves_unexported_zdotdir_and_loads_before_prompt() {
            let Some(zsh) = ["/bin/zsh", "/usr/bin/zsh"]
                .into_iter()
                .find(|path| Path::new(path).is_file())
            else {
                return;
            };
            let home = tempfile::tempdir().unwrap();
            let zdir = home.path().join("custom-zsh");
            std::fs::write(
                home.path().join(".zshenv"),
                format!("ZDOTDIR='{}'\n", zdir.display()),
            )
            .unwrap();
            let list = home.path().join("shells-list");
            std::fs::write(&list, format!("{zsh}\n")).unwrap();
            let command = installation(
                "1.0.0",
                ShellIntegration {
                    bash: "",
                    zsh: "printf 'ZSH_READY\\n'\n",
                },
                list.to_str().unwrap(),
            );
            ready(
                Command::new(zsh)
                    .args(["-c", &command])
                    .env("HOME", home.path())
                    .env("SHELL", zsh)
                    .env_remove("ZDOTDIR")
                    .output()
                    .unwrap(),
            );
            assert!(!home.path().join(".bashrc").exists());
            assert!(zdir.join(".zshrc").is_file());
            let output = Command::new(zsh)
                .args(["-d", "-ic", "history; :"])
                .env("HOME", home.path())
                .env_remove("ZDOTDIR")
                .env_remove("ALACRTERM_REMOTE_INTEGRATION")
                .env("LC_ALACRTERM_INTEGRATION", "1")
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(String::from_utf8_lossy(&output.stdout), "ZSH_READY\n");
        }
        #[test]
        fn startup_loads_without_history_and_only_for_marked_sessions() {
            let home = tempfile::tempdir().unwrap();
            ready(install(
                home.path(),
                "/bin/bash\n",
                "1.0.0",
                "printf 'INTEGRATION_LOADED\\n'\n",
            ));
            for enabled in [true, false] {
                let mut command = Command::new("bash");
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
                let output = command.output().unwrap();
                assert!(output.status.success());
                assert_eq!(
                    String::from_utf8_lossy(&output.stdout),
                    if enabled { "INTEGRATION_LOADED\n" } else { "" }
                );
            }
        }
    }
}
