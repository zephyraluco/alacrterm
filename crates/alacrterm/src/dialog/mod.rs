//! 应用里的对话框（`Window::open_dialog` 的浮层，不是独立窗口）：一个对话框一个文件，
//! 都只改会话列表、不碰终端实例；触发点走 [`crate::actions`] 的 action，列表改动落在
//! [`crate::sidebar_panel::sessions::SessionsState`] 实体上（实体句柄随对话框一起传入）。
//!
//! 共同约定：
//! - 表单实体必须在打开对话框**之前**建（构建闭包是 `Fn`，每帧都会被调用）；
//! - 页脚用 `DialogFooter` + `DialogClose` / `DialogAction` 自拼；
//! - `on_ok` 做兜底校验并写列表实体；只有需要窗口的动作才配
//!   [`crate::AppRoot::defer_after_update`]。

mod connection;
mod folder;
