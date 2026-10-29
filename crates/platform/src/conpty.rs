//! Windows ConPTY 后端选择：优先随包的 Windows Terminal ConPTY，否则退回系统自带。

use std::path::{Path, PathBuf};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetDllDirectoryW(lp_path_name: *const u16) -> i32;
    fn LoadLibraryW(lp_file_name: *const u16) -> *mut core::ffi::c_void;
    fn FreeLibrary(h_lib_module: *mut core::ffi::c_void) -> i32;
}

fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt as _;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// 预加载 `dir/conpty.dll`。
fn preload(dir: &Path) -> bool {
    let dll = dir.join("conpty.dll");
    if !dll.is_file() {
        return false;
    }
    let handle = unsafe { LoadLibraryW(wide(&dll).as_ptr()) };
    if handle.is_null() {
        return false;
    }
    unsafe { FreeLibrary(handle) };
    true
}

/// conpty 目录的候选位置。
fn candidate_dirs(exe_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![exe_dir.join("plugins/conpty")];
    for up in 2..=3 {
        if let Some(root) = exe_dir.ancestors().nth(up) {
            dirs.push(root.join("plugins/windows/conpty"));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join("plugins/windows/conpty"));
    }
    dirs
}

/// 选择 ConPTY 后端。
pub fn ensure() {
    let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    else {
        log::warn!("conpty: 取不到 exe 路径，跳过 WT ConPTY 探测");
        return;
    };

    if exe_dir.join("OpenConsole.exe").is_file() && preload(&exe_dir) {
        log::info!("conpty: 使用 exe 同级的 conpty.dll（Windows Terminal ConPTY）");
        return;
    }

    for dir in candidate_dirs(&exe_dir) {
        if !dir.join("conpty.dll").is_file() || !dir.join("OpenConsole.exe").is_file() {
            continue;
        }
        unsafe { SetDllDirectoryW(wide(&dir).as_ptr()) };
        if preload(&dir) {
            log::info!("conpty: 使用 {}（Windows Terminal ConPTY）", dir.display());
            return;
        }
    }

    let message = "conpty: 未找到 Windows Terminal 的 conpty.dll（应在 exe 同级或 \
                   plugins/conpty/ 或源码 plugins/windows/conpty/），退回系统 ConPTY：窗口缩到极小再放大时可能丢失上方内容";
    log::warn!("{message}");
    eprintln!("[conpty] {message}");
}
