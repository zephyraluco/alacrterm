#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    let result = std::env::args_os()
        .nth(1)
        .ok_or_else(|| "缺少更新请求文件".to_owned())
        .and_then(|path| {
            updater::windows::run(std::path::Path::new(&path)).map_err(|error| format!("{error:#}"))
        });
    if let Err(error) = result {
        if let Ok(folder) = updater::data_dir() {
            if let Err(logging) = std::fs::create_dir_all(&folder)
                .and_then(|()| std::fs::write(folder.join("helper-error.log"), &error))
            {
                eprintln!("{logging}: {error}");
            }
        }
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {}
