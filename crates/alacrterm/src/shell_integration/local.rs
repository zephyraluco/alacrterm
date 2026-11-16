//! Session-scoped PowerShell integration. Other local shells start unmodified.
use anyhow::Result;
use std::{collections::HashMap, path::Path};
use tempfile::TempDir;

pub fn prepare(
    program: &str,
    args: &[String],
    env: &mut HashMap<String, String>,
) -> Result<Option<(Vec<String>, TempDir)>> {
    let name = Path::new(program)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(name.as_str(), "pwsh" | "powershell")
        || !args.is_empty()
        || env
            .get("ALACRTERM_SHELL_INTEGRATION")
            .is_some_and(|value| value == "0")
    {
        return Ok(None);
    }
    let dir = tempfile::Builder::new()
        .prefix("alacrterm-shell-")
        .tempdir()?;
    let script = dir.path().join("integration.ps1");
    let contents = format!(
        "{}\n{}",
        include_str!("../../../../scripts/shell_integration/history.ps1"),
        include_str!("../../../../scripts/shell_integration/powershell.ps1"),
    );
    std::fs::write(&script, contents.replace("\r\n", "\n"))?;
    env.insert(
        "ALACRTERM_INTEGRATION_SCRIPT".into(),
        script.to_string_lossy().into_owned(),
    );
    Ok(Some((
        vec![
            "-NoExit".into(),
            "-File".into(),
            script.to_string_lossy().into_owned(),
        ],
        dir,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_and_zsh_start_without_integration_or_environment_changes() {
        for program in [
            "bash",
            "/bin/bash",
            "zsh",
            "/bin/zsh",
            "bash.exe",
            "zsh.exe",
        ] {
            let mut env = HashMap::from([("ZDOTDIR".into(), "/custom/zsh".into())]);
            let before = env.clone();
            assert!(prepare(program, &[], &mut env).unwrap().is_none());
            assert_eq!(env, before);
        }
    }

    #[test]
    fn powershell_keeps_integration_and_respects_custom_arguments_and_opt_out() {
        for program in ["pwsh", "powershell", "pwsh.exe", "powershell.exe"] {
            let mut env = HashMap::new();
            let (args, lease) = prepare(program, &[], &mut env).unwrap().unwrap();
            assert_eq!(&args[..2], &["-NoExit", "-File"]);
            let script = Path::new(&args[2]);
            assert!(script.is_file());
            assert!(
                std::fs::read_to_string(script)
                    .unwrap()
                    .contains("PSReadLine")
            );
            assert_eq!(env["ALACRTERM_INTEGRATION_SCRIPT"], args[2]);
            drop(lease);
            assert!(!script.exists());
            let mut env = HashMap::new();
            assert!(
                prepare(program, &["-NoProfile".into()], &mut env)
                    .unwrap()
                    .is_none()
            );
            assert!(env.is_empty());
            env.insert("ALACRTERM_SHELL_INTEGRATION".into(), "0".into());
            let before = env.clone();
            assert!(prepare(program, &[], &mut env).unwrap().is_none());
            assert_eq!(env, before);
        }
    }
}
