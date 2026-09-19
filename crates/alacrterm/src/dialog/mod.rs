//! 应用里的**对话框**（`Window::open_dialog` 的浮层，不是独立窗口）。
//!
//! 一个对话框一个文件，都只做一件事，且**都不碰终端实例**：
//! - [`connection`]：「新建会话」——收集 SSH 参数，往会话列表里加一条**记录**；
//! - [`folder`]：「新建文件夹」——给会话列表建一个分组文件夹。
//!
//! 两个入口的触发点都在侧边栏（状态栏两端的两枚按钮 / 文件夹行的右键菜单），
//! 统一走 [`crate::actions`] 的 action；真正的列表改动落在
//! [`crate::sidebar_panel::sessions`] 的 `AppRoot` 方法上。
//!
//! 共同约定：
//! - **表单实体必须在打开对话框之前创建**（构建闭包是 `Fn`，每帧都会被调用，
//!   在闭包内建 `InputState` 会把输入每帧重置）；
//! - 页脚自己用 `DialogFooter` + `DialogClose` / `DialogAction` 拼（`Dialog`
//!   不会自动生成确定 / 取消按钮）；
//! - `on_ok` 里既做**兜底校验**（必填项缺失就 `return false`，不关对话框），
//!   也负责把结果交给根视图 —— 动作回调期间窗口仍在更新栈上，所以要配
//!   [`crate::AppRoot::defer_after_update`] 让出一拍。

mod connection;
mod folder;
