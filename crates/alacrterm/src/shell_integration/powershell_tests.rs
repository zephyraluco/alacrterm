//! Runs on Windows CI, and on Unix when a PowerShell executable is available.
#[test]
fn psreadline_history_handles_custom_paths_appends_truncation_and_async_delivery() {
    let program = std::env::var_os("ALACRTERM_TEST_PWSH")
        .unwrap_or_else(|| if cfg!(windows) { "powershell" } else { "pwsh" }.into());
    if !cfg!(windows)
        && std::process::Command::new(&program)
            .arg("-Version")
            .output()
            .is_err()
    {
        return;
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let work = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(program)
        .args(["-NoProfile", "-NonInteractive", "-File"])
        .arg(root.join("scripts/test/test_shell_history.ps1"))
        .arg("-HistoryScript")
        .arg(root.join("scripts/shell_integration/history.ps1"))
        .arg("-WorkDirectory")
        .arg(work.path())
        .env("XDG_CACHE_HOME", work.path().join("cache"))
        .env("XDG_DATA_HOME", work.path().join("data"))
        .env("XDG_CONFIG_HOME", work.path().join("config"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("HISTORY_WORKER_OK"));
}
