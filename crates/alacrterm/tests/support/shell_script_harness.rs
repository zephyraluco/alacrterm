//! Test harness for the scripts used by SSH integration; never used for local startup.
use anyhow::Result;
use std::{collections::HashMap, path::Path};
use tempfile::TempDir;

pub fn prepare(
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
            include_str!("../../../../scripts/shell_integration/bash.sh"),
        ),
        "zsh" => (
            "integration.zsh",
            include_str!("../../../../scripts/shell_integration/zsh.zsh"),
        ),
        _ => (
            "integration.ps1",
            include_str!("../../../../scripts/shell_integration/powershell.ps1"),
        ),
    };
    let script = dir.path().join(script);
    // Embedded scripts may come from a CRLF checkout on Windows.
    let contents = if matches!(name.as_str(), "bash" | "zsh") {
        format!(
            "{}\n{contents}",
            include_str!("../../../../scripts/shell_integration/history.sh")
        )
    } else {
        format!(
            "{}\n{contents}",
            include_str!("../../../../scripts/shell_integration/history.ps1")
        )
    };
    std::fs::write(&script, contents.replace("\r\n", "\n"))?;
    env.insert(
        "ALACRTERM_INTEGRATION_SCRIPT".into(),
        script.to_string_lossy().into_owned(),
    );
    let args = match name.as_str() {
        "bash" => {
            let rc = dir.path().join("bashrc");
            std::fs::write(
                &rc,
                "[ -r \"$HOME/.bashrc\" ] && . \"$HOME/.bashrc\"\n. \"$ALACRTERM_INTEGRATION_SCRIPT\"\n",
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
