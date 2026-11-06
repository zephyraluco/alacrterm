//! Install the verified Debian package with system authorization, before restart.
use anyhow::{Context as _, Result, ensure};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, Output},
};

#[derive(Debug)]
pub struct Installer {
    pub program: PathBuf,
    pub arguments: Vec<OsString>,
}

fn installer(package: &Path) -> Installer {
    Installer {
        program: "/usr/bin/pkexec".into(),
        arguments: vec![
            "/usr/bin/apt-get".into(),
            "install".into(),
            "--yes".into(),
            "--".into(),
            package.as_os_str().to_owned(),
        ],
    }
}

fn run_installer(
    command: Installer,
    run: impl FnOnce(&Path, &[OsString]) -> std::io::Result<Output>,
) -> Result<()> {
    let output = run(&command.program, &command.arguments).context("无法启动系统更新安装器")?;
    ensure!(
        output.status.success(),
        "安装未完成（可能已取消系统授权）：{}",
        String::from_utf8_lossy(&output.stderr)
            .chars()
            .take(4096)
            .collect::<String>()
    );
    Ok(())
}

/// Run on a worker thread. A failed or canceled install must never trigger restart.
pub fn install(package: &Path) -> Result<()> {
    ensure!(
        Path::new("/usr/bin/pkexec").is_file() && Path::new("/usr/bin/apt-get").is_file(),
        "自动安装需要 PolicyKit（pkexec）及 apt-get，请安装后重试"
    );
    run_installer(installer(package), |program, arguments| {
        Command::new(program).args(arguments).output()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_paths_are_literal_arguments_not_shell_commands() {
        let path = Path::new("/home/Alice Smith/a'b;$(touch ignored).deb");
        let command = installer(path);
        assert_eq!(command.program, Path::new("/usr/bin/pkexec"));
        assert_eq!(
            command.arguments,
            vec![
                OsString::from("/usr/bin/apt-get"),
                "install".into(),
                "--yes".into(),
                "--".into(),
                path.as_os_str().to_owned(),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn install_failure_or_canceled_authorization_never_reports_success() {
        use std::os::unix::process::ExitStatusExt as _;
        for code in [0, 1, 126, 127] {
            let result = run_installer(installer(Path::new("/cache/update.deb")), |_, _| {
                Ok(Output {
                    status: std::process::ExitStatus::from_raw(code << 8),
                    stdout: Vec::new(),
                    stderr: b"authorization canceled".to_vec(),
                })
            });
            assert_eq!(result.is_ok(), code == 0);
        }
        assert!(
            run_installer(installer(Path::new("/cache/update.deb")), |_, _| {
                Err(std::io::Error::from(std::io::ErrorKind::NotFound))
            })
            .is_err()
        );
    }
}
