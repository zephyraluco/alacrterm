//! Windows ConPTY 后端选择：优先用 Windows Terminal 的 OpenConsole。
//!
//! `alacritty_terminal` 建伪控制台时先 `LoadLibraryW("conpty.dll")`：命中就用 WT 的
//! ConPTY（`OpenConsole.exe`），否则退回 Windows 自带的那套 —— 后者在窗口纵向缩到极小
//! 再放大时会让壳侧整片重绘、**丢失上方内容**（实测可见区差 12795 像素；换成 OpenConsole
//! 后同操作 0 像素差异）

use std::path::{Path, PathBuf};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetDllDirectoryW(lp_path_name: *const u16) -> i32;
    fn LoadLibraryW(lp_file_name: *const u16) -> *mut core::ffi::c_void;
    fn FreeLibrary(h_lib_module: *mut core::ffi::c_void) -> i32;
}

fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt as _;
    path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

/// 按全路径预加载 `dir/conpty.dll`：加载器按模块基名去重，所以之后
/// `alacritty_terminal` 那句 `LoadLibraryW("conpty.dll")` 会拿到同一份。
fn preload(dir: &Path) -> bool {
    let dll = dir.join("conpty.dll");
    if !dll.is_file() {
        return false;
    }
    let handle = unsafe { LoadLibraryW(wide(&dll).as_ptr()) };
    if handle.is_null() {
        return false;
    }
    // 只是占位：模块已留在进程里，后续 LoadLibraryW 仍会命中它
    unsafe { FreeLibrary(handle) };
    true
}

/// 开发形态下 conpty 目录的候选位置（`cargo run` 时 exe 在 `target/<profile>/`）。
fn candidate_dirs(exe_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    // target/debug 或 target/<triple>/debug → 仓库根
    for up in 2..=3 {
        if let Some(root) = exe_dir.ancestors().nth(up) {
            dirs.push(root.join("assets/windows/conpty"));
        }
    }
    // cargo run 的 cwd 通常是仓库根
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join("assets/windows/conpty"));
    }
    dirs
}

/// 让随包分发的 WT ConPTY 被 `alacritty_terminal` 选中：先看 exe 同级（发行形态），
/// 再回退到仓库 `assets/windows/conpty/`（开发形态），都没有就告警并用系统 ConPTY。
pub fn ensure() {
    let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    else {
        log::warn!("conpty: 取不到 exe 路径，跳过 WT ConPTY 探测");
        return;
    };

    // 1) 发行形态：exe 旁边就有
    if preload(&exe_dir) {
        log::info!("conpty: 使用 exe 同级的 conpty.dll（Windows Terminal ConPTY）");
        return;
    }

    // 2) 开发形态：仓库里的 assets
    for dir in candidate_dirs(&exe_dir) {
        if !dir.join("conpty.dll").is_file() || !dir.join("OpenConsole.exe").is_file() {
            continue;
        }
        // 只影响之后的 LoadLibrary 搜索路径；PTY 是建终端时才创建的，故此处安全
        unsafe { SetDllDirectoryW(wide(&dir).as_ptr()) };
        if preload(&dir) {
            log::info!("conpty: 使用 {}（Windows Terminal ConPTY）", dir.display());
            return;
        }
    }

    // 3) 退回系统 ConPTY
    let message = "conpty: 未找到 Windows Terminal 的 conpty.dll（应在 exe 同级或 \
                   assets/windows/conpty/），退回系统 ConPTY：窗口缩到极小再放大时可能丢失上方内容";
    log::warn!("{message}");
    eprintln!("[conpty] {message}");
}
