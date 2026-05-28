use alacritty_terminal::tty;
use std::env;

/// 在 PATH 中查找可执行文件，返回完整路径
pub fn find_in_env(filename: &str) -> Option<String> {
    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths).find_map(|mut dir| {
            dir.push(filename);
            if dir.is_file() {
                dir.to_str().map(|s| s.to_string())
            } else {
                None
            }
        })
    })
}

/// 查找系统 Shell 程序，Windows 优先 PowerShell 7，然后 PowerShell 5
pub fn find_shell() -> Option<tty::Shell> {
    #[cfg(target_os = "windows")]
    {
        // 优先使用 PowerShell 7 (pwsh.exe)
        if let Some(path) = find_in_env("pwsh.exe") {
            return Some(tty::Shell::new(path, vec![]));
        }
        // 回退到 PowerShell 5 (powershell.exe)
        if let Some(path) = find_in_env("powershell.exe") {
            return Some(tty::Shell::new(path, vec![]));
        }
        // 最后回退到 cmd.exe
        if let Some(path) = find_in_env("cmd.exe") {
            return Some(tty::Shell::new(path, vec![]));
        }
        None
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Unix: 读取 SHELL 环境变量
        if let Ok(shell) = env::var("SHELL") {
            return Some(tty::Shell::new(shell, vec![]));
        }
        // 回退到 bash
        if let Some(path) = find_in_env("bash") {
            return Some(tty::Shell::new(path, vec![]));
        }
        None
    }
}
