//! Installation requests carry their original signed envelope across the process boundary.
use crate::{
    download,
    manifest::{self, Offer},
};
use anyhow::{Context as _, Result, ensure};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
pub struct InstallRequest {
    pub envelope: Vec<u8>,
    pub package: PathBuf,
    pub executable: PathBuf,
    pub from: Version,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct InstallResult {
    pub from: Version,
    pub to: Version,
    pub installed: bool,
    pub message: String,
}

impl InstallRequest {
    pub fn verified_release(&self, keys: &[String]) -> Result<manifest::Release> {
        ensure!(
            self.executable.is_absolute()
                && self
                    .executable
                    .file_name()
                    .is_some_and(|name| name == "alacrterm.exe"),
            "更新目标不是 Alacrterm 安装程序"
        );
        ensure!(self.package.is_absolute(), "安装包路径必须是绝对路径");
        let manifest = manifest::open(&self.envelope, keys)?;
        let Offer::Update(release) = manifest.offer(&self.from, "windows-x64")? else {
            anyhow::bail!("没有可安装的 Windows 更新");
        };
        download::verify(&self.package, &release.asset)?;
        Ok(release)
    }
}

pub fn installer_arguments(directory: &Path, log: &Path) -> Vec<OsString> {
    let mut destination = OsString::from("/DIR=");
    destination.push(directory);
    let mut logging = OsString::from("/LOG=");
    logging.push(log);
    vec![
        "/VERYSILENT".into(),
        "/SUPPRESSMSGBOXES".into(),
        "/NORESTART".into(),
        "/SP-".into(),
        "/NOCLOSEAPPLICATIONS".into(),
        destination,
        logging,
    ]
}

pub fn prepare(envelope: Vec<u8>, package: PathBuf, current: Version) -> Result<PathBuf> {
    let executable = std::env::current_exe()?;
    let helper = executable
        .parent()
        .context("程序没有安装目录")?
        .join("alacrterm-update-helper.exe");
    ensure!(helper.is_file(), "安装目录缺少更新助手，请重新下载安装包");
    let request = InstallRequest {
        envelope,
        package,
        executable,
        from: current,
    };
    let release = request.verified_release(&crate::trusted_keys())?;
    let folder = crate::cache_dir()?.join(format!("install-{}", release.version));
    std::fs::create_dir_all(&folder)?;
    let copied_helper = folder.join("alacrterm-update-helper.exe");
    std::fs::copy(helper, &copied_helper)?;
    let plan = folder.join("request.json");
    util::atomic_file::write(&plan, &serde_json::to_vec(&request)?)?;
    Ok(plan)
}

#[cfg(windows)]
pub fn start(plan: &Path) -> Result<std::process::Child> {
    use std::os::windows::process::CommandExt as _;
    Ok(std::process::Command::new(
        plan.parent()
            .context("缺少更新目录")?
            .join("alacrterm-update-helper.exe"),
    )
    .arg(plan)
    .creation_flags(0x08000000)
    .spawn()?)
}

#[cfg(any(windows, test))]
fn installation_process(path: &Path, executable: &Path) -> bool {
    fn normalized(path: &Path) -> String {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_owned());
        canonical
            .to_string_lossy()
            .replace('/', "\\")
            .trim_start_matches("\\\\?\\")
            .to_ascii_lowercase()
    }
    let path = normalized(path);
    let executable = normalized(executable);
    path == executable
        || executable
            .rsplit_once('\\')
            .is_some_and(|(directory, _)| path.starts_with(&format!("{directory}\\plugins\\")))
}

#[cfg(windows)]
pub fn run(plan: &Path) -> Result<()> {
    use std::{
        fs,
        os::windows::process::CommandExt as _,
        time::{Duration, Instant},
    };
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let request: InstallRequest = serde_json::from_slice(&fs::read(plan)?)?;
    let release = request.verified_release(&crate::trusted_keys())?;
    let directory = request.executable.parent().context("缺少安装目录")?;
    let log_folder = crate::data_dir()?;
    fs::create_dir_all(&log_folder)?;
    let result_path = log_folder.join("result.json");
    let mut system = System::new();
    let waiting = Instant::now();
    // Wait for every instance and owned WayX process, rather than only the requesting PID.
    loop {
        system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
        );
        let active = system.processes().values().any(|process| {
            process
                .exe()
                .is_some_and(|path| installation_process(path, &request.executable))
        });
        if !active {
            break;
        }
        if waiting.elapsed() > Duration::from_secs(120) {
            let result = InstallResult {
                from: request.from,
                to: release.version,
                installed: false,
                message: "应用或 WayX 未退出，更新已取消".into(),
            };
            util::atomic_file::write(&result_path, &serde_json::to_vec(&result)?)?;
            anyhow::bail!("等待应用退出超时");
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    // Revalidate immediately before handing control to executable code.
    request.verified_release(&crate::trusted_keys())?;
    let status = std::process::Command::new(&request.package)
        .args(installer_arguments(
            directory,
            &log_folder.join("installer.log"),
        ))
        .creation_flags(0x08000000)
        .status();
    let (installed, message) = match status {
        Ok(status) if status.success() => (true, "安装完成，等待新版本启动确认".into()),
        Ok(status) => (
            false,
            format!("安装程序退出码：{:?}；请查看 installer.log", status.code()),
        ),
        Err(error) => (false, format!("无法启动安装程序：{error}")),
    };
    let result = InstallResult {
        from: request.from,
        to: release.version,
        installed,
        message,
    };
    util::atomic_file::write(&result_path, &serde_json::to_vec(&result)?)?;
    // If installation failed, an intact old version may still be relaunched.
    std::process::Command::new(&request.executable)
        .creation_flags(0x08000000)
        .spawn()
        .context("安装后启动应用失败")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn process_waiting_matches_case_and_extended_paths_without_matching_other_installs() {
        let executable = Path::new("C:/Users/Alice/Alacrterm/alacrterm.exe");
        assert!(installation_process(
            Path::new(r"\\?\C:\Users\Alice\Alacrterm\ALACRTERM.EXE"),
            executable
        ));
        assert!(installation_process(
            Path::new("C:/Users/Alice/Alacrterm/plugins/windows/bin/wayx.exe"),
            executable
        ));
        assert!(!installation_process(
            Path::new("C:/Users/Bob/Alacrterm/alacrterm.exe"),
            executable
        ));
        assert!(!installation_process(
            Path::new("C:/Users/Alice/Alacrterm/plugins-other/wayx.exe"),
            executable
        ));
    }

    #[test]
    fn installer_paths_remain_separate_arguments_without_shell_quoting() {
        let directory = Path::new("C:/Users/Alice Smith/Programs/Alacrterm");
        let log = Path::new("C:/Users/Alice Smith/update.log");
        let args = installer_arguments(directory, log);
        assert!(args.contains(&OsString::from(
            "/DIR=C:/Users/Alice Smith/Programs/Alacrterm"
        )));
        assert!(args.contains(&OsString::from("/LOG=C:/Users/Alice Smith/update.log")));
        assert!(args.contains(&OsString::from("/NORESTART")));
    }
    #[test]
    fn helper_revalidates_signed_metadata_and_package_at_install_time() {
        use sha2::{Digest as _, Sha256};
        use std::io::Cursor;
        let folder = tempfile::tempdir().unwrap();
        let package = folder.path().join("setup.exe");
        std::fs::write(&package, b"installer").unwrap();
        let manifest = serde_json::json!({
            "schema": 1, "channel": "stable", "version": "0.2.0", "notes": "test",
            "assets": { "windows-x64": { "url": "https://example.com/setup.exe", "size": 9,
                "sha256": format!("{:x}", Sha256::digest(b"installer")), "kind": "exe" } }
        })
        .to_string();
        let key = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let signature = minisign::sign(
            Some(&key.pk),
            &key.sk,
            Cursor::new(manifest.as_bytes()),
            Some("alacrterm-manifest stable 0.2.0"),
            None,
        )
        .unwrap()
        .into_string();
        let mut request = InstallRequest {
            envelope: serde_json::to_vec(&manifest::Envelope {
                manifest,
                signature,
            })
            .unwrap(),
            package: package.clone(),
            executable: folder.path().join("install/alacrterm.exe"),
            from: Version::new(0, 1, 0),
        };
        let keys = vec![key.pk.to_base64()];
        assert_eq!(
            request.verified_release(&keys).unwrap().version,
            Version::new(0, 2, 0)
        );
        request.from = Version::new(0, 2, 0);
        assert!(request.verified_release(&keys).is_err());
        request.from = Version::new(0, 1, 0);
        std::fs::write(package, b"tampered!").unwrap();
        assert!(request.verified_release(&keys).is_err());
    }

    #[test]
    fn helper_rejects_unsigned_install_requests() {
        let request = InstallRequest {
            envelope: b"{}".to_vec(),
            package: PathBuf::from("/cache/setup.exe"),
            executable: PathBuf::from("/app/alacrterm.exe"),
            from: Version::new(0, 1, 0),
        };
        assert!(request.verified_release(&[]).is_err());
    }
}
