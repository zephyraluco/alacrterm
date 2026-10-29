#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    let _logging = util::logging::init("alacrterm-update-helper");
    let result = std::env::args_os()
        .nth(1)
        .ok_or_else(|| "缺少更新请求文件".to_owned())
        .and_then(|path| {
            updater::windows::run(std::path::Path::new(&path)).map_err(|error| format!("{error:#}"))
        });
    if let Err(error) = result {
        if let Some(folder) = std::env::args_os().nth(1).and_then(|path| {
            std::path::PathBuf::from(path)
                .parent()
                .map(std::path::Path::to_owned)
        }) {
            if let Err(logging) = std::fs::create_dir_all(&folder)
                .and_then(|()| std::fs::write(folder.join("helper-error.log"), &error))
            {
                eprintln!("{logging}: {error}");
            }
        }
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}

#[cfg(not(windows))]
fn main() {}
