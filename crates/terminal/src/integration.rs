//! Session-scoped startup wrappers. Custom shell arguments keep their original behavior.
use anyhow::Result;
use std::{collections::HashMap, path::Path};
use tempfile::TempDir;

pub(crate) fn prepare(
    program: &str,
    args: &[String],
    env: &mut HashMap<String, String>,
) -> Result<Option<(Vec<String>, TempDir)>> {
    if !args.is_empty()
        || env
            .get("ALACRTERM_SHELL_INTEGRATION")
            .is_some_and(|value| value == "0")
    {
        return Ok(None);
    }
    let name = Path::new(program)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(name.as_str(), "bash" | "zsh" | "pwsh" | "powershell") {
        return Ok(None);
    }
    let dir = tempfile::Builder::new()
        .prefix("alacrterm-shell-")
        .tempdir()?;
    let (script, contents) = match name.as_str() {
        "bash" => (
            "integration.sh",
            include_str!("../shell-integration/bash.sh"),
        ),
        "zsh" => (
            "integration.zsh",
            include_str!("../shell-integration/zsh.zsh"),
        ),
        _ => (
            "integration.ps1",
            include_str!("../shell-integration/powershell.ps1"),
        ),
    };
    let script = dir.path().join(script);
    std::fs::write(&script, contents)?;
    env.insert(
        "ALACRTERM_INTEGRATION_SCRIPT".into(),
        script.to_string_lossy().into_owned(),
    );
    let args = match name.as_str() {
        "bash" => {
            let rc = dir.path().join("bashrc");
            std::fs::write(
                &rc,
                "[[ -r $HOME/.bashrc ]] && source \"$HOME/.bashrc\"\nsource \"$ALACRTERM_INTEGRATION_SCRIPT\"\n",
            )?;
            vec![
                "--rcfile".into(),
                rc.to_string_lossy().into_owned(),
                "-i".into(),
            ]
        }
        "zsh" => {
            let original = env
                .get("ZDOTDIR")
                .cloned()
                .or_else(|| env.get("HOME").cloned())
                .unwrap_or_default();
            env.insert("ALACRTERM_ORIGINAL_ZDOTDIR".into(), original);
            std::fs::write(
                dir.path().join(".zshenv"),
                "typeset __alacrterm_wrapper_dir=$ZDOTDIR\nZDOTDIR=$ALACRTERM_ORIGINAL_ZDOTDIR\n[[ -r $ZDOTDIR/.zshenv ]] && source \"$ZDOTDIR/.zshenv\"\nALACRTERM_ORIGINAL_ZDOTDIR=${ZDOTDIR:-$HOME}\nZDOTDIR=$__alacrterm_wrapper_dir\nunset __alacrterm_wrapper_dir\n",
            )?;
            std::fs::write(
                dir.path().join(".zshrc"),
                "ZDOTDIR=$ALACRTERM_ORIGINAL_ZDOTDIR\n[[ -r $ZDOTDIR/.zshrc ]] && source \"$ZDOTDIR/.zshrc\"\nsource \"$ALACRTERM_INTEGRATION_SCRIPT\"\n",
            )?;
            env.insert("ZDOTDIR".into(), dir.path().to_string_lossy().into_owned());
            vec!["-i".into()]
        }
        _ => vec![
            "-NoExit".into(),
            "-File".into(),
            script.to_string_lossy().into_owned(),
        ],
    };
    Ok(Some((args, dir)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn custom_shell_arguments_are_preserved() {
        assert!(
            prepare("bash", &["-c".into(), "echo x".into()], &mut HashMap::new())
                .unwrap()
                .is_none()
        );
    }
    #[test]
    #[cfg(unix)]
    fn bash_startup_preserves_rc_and_emits_lifecycle_markers() {
        use std::{
            io::Write as _,
            process::{Command, Stdio},
        };
        if !Path::new("/bin/bash").exists() {
            return;
        }
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join(".bashrc"),
            "printf 'PROFILE_LOADED\\n'\nPS1='test$ '\n",
        )
        .unwrap();
        let mut env = HashMap::from([("HOME".into(), home.path().to_string_lossy().into_owned())]);
        let (args, _files) = prepare("/bin/bash", &[], &mut env).unwrap().unwrap();
        let mut child = Command::new("/bin/bash")
            .args(args)
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"echo worked\nexit\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let mut bytes = output.stdout;
        bytes.extend(output.stderr);
        assert!(
            bytes
                .windows(b"PROFILE_LOADED".len())
                .any(|part| part == b"PROFILE_LOADED")
        );
        assert!(
            bytes
                .windows(b"\x1b]633;B\x07".len())
                .any(|part| part == b"\x1b]633;B\x07")
        );
        assert!(
            bytes
                .windows(b"\x1b]633;C\x07".len())
                .any(|part| part == b"\x1b]633;C\x07")
        );
    }
    #[test]
    #[cfg(unix)]
    fn zsh_startup_preserves_user_files_and_emits_lifecycle_markers() {
        use std::{
            io::Write as _,
            process::{Command, Stdio},
        };
        let program = if Path::new("/usr/bin/zsh").exists() {
            "/usr/bin/zsh"
        } else {
            return;
        };
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(".zshenv"), "printf 'USER_ZSHENV\\n'\n").unwrap();
        std::fs::write(home.path().join(".zshrc"), "PROMPT='test%% '\n").unwrap();
        let mut env = HashMap::from([("HOME".into(), home.path().to_string_lossy().into_owned())]);
        let (args, _files) = prepare(program, &[], &mut env).unwrap().unwrap();
        let mut child = Command::new(program)
            .args(args)
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"echo worked\nexit\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let mut bytes = output.stdout;
        bytes.extend(output.stderr);
        assert_eq!(
            String::from_utf8_lossy(&bytes)
                .matches("USER_ZSHENV")
                .count(),
            1
        );
        assert!(
            bytes
                .windows(b"\x1b]633;B\x07".len())
                .any(|part| part == b"\x1b]633;B\x07")
        );
        assert!(
            bytes
                .windows(b"\x1b]633;C\x07".len())
                .any(|part| part == b"\x1b]633;C\x07")
        );
    }
}
