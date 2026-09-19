//! 应用自定义 Action：供右键菜单等「官方 action 风格」的菜单项使用。
//!
//! 按 gpui-kit 官方示例，菜单项写作
//! `menu.menu("标签", Box::new(SomeAction))`，点击后由菜单内部
//! `window.dispatch_action(...)` 派发。接收方有两种写法：
//!
//! - **元素级** `element.on_action(cx.listener(|this, action: &A, window, cx| ...))`：
//!   沿「焦点路径」派发，依赖当前焦点位置；
//! - **全局级** [`App::on_action`]（本模块采用）：在 action 冒泡阶段**一定**会被调用，
//!   与焦点无关。
//!
//! 菜单是**同一窗口内的浮层**（`ContextMenuExt` 用 `deferred` 渲染），但它派发 action 时
//! 若没有设置 `action_context`，焦点可能仍在终端上；用全局监听器可避免这层不确定性。
//! 代价是全局监听器只有 `&mut App`（没有窗口、也没有根视图），因此回调里要配合
//! [`AppRoot::defer_after_update`] 才能做「需要窗口」的操作。
//!
//! 本模块的 action 与两条线的对应关系（见 [`crate::main`] 模块文档）：
//! - **会话记录**：[`NewSession`]（弹建连对话框，加一条记录）、[`NewFolder`]
//!   （弹输入框，加一个文件夹）、[`OpenSession`]（双击记录，按它开一个终端）、
//!   [`MoveEntry`]（把条目拖到别的文件夹 / 拖回顶层）、[`RemoveEntry`]（删掉记录或文件夹本身）；
//! - **终端会话**：[`NewLocalTerminal`]（标签栏 `+` / 欢迎页：直接开一个本地终端）。
//!
//! ⚠️ 带数据的 action **不能**写进 [`actions!`] 宏：宏会生成同名的 unit 结构体，
//! 与下面手写的定义撞名（E0428）。

use gpui::{Action, App, WeakEntity, actions};
use serde::Deserialize;

use crate::AppRoot;
use crate::sidebar_panel::sessions::SessionPath;

actions!(alacrterm, [NewLocalTerminal, OpenSettings]);

/// 新建一条会话记录（弹建连对话框）。
///
/// `folder` 是「放在哪个文件夹里」：`None` = **顶层**（状态栏右下角的 `+` 就是这个），
/// `Some(path)` = 文件夹行的右键菜单「在这里新建会话」。
#[derive(Clone, Action, PartialEq, Eq, Deserialize)]
#[action(namespace = alacrterm, no_json)]
pub(crate) struct NewSession {
    pub(crate) folder: Option<SessionPath>,
}

/// 新建一个文件夹。
///
/// `parent` 是「建在哪个文件夹下」：`None` = 顶层（状态栏左侧的文件夹图标），
/// `Some(path)` = 文件夹行的右键菜单「新建子文件夹」。
#[derive(Clone, Action, PartialEq, Eq, Deserialize)]
#[action(namespace = alacrterm, no_json)]
pub(crate) struct NewFolder {
    pub(crate) parent: Option<SessionPath>,
}

/// 按某条**记录**新建一个终端（双击该行 / 右键「打开会话」）。
///
/// 记录本身不受影响：同一条记录可以开任意多个终端，关掉终端也不会删掉记录。
#[derive(Clone, Action, PartialEq, Eq, Deserialize)]
#[action(namespace = alacrterm, no_json)]
pub(crate) struct OpenSession {
    pub(crate) path: SessionPath,
}

/// 把列表里的一个条目（记录 / 文件夹）挪到另一个目录下（拖动放下）。
///
/// `into` = 目标**文件夹**路径：`None` = 顶层（拖到列表下方空白处）。
/// 目标若在被拖动项自己的子树里（含它本身）会被忽略——否则会把子树拖成一个环。
#[derive(Clone, Action, PartialEq, Eq, Deserialize)]
#[action(namespace = alacrterm, no_json)]
pub(crate) struct MoveEntry {
    pub(crate) from: SessionPath,
    pub(crate) into: Option<SessionPath>,
}

/// 删除列表里的一个条目（记录 / 文件夹）。
///
/// 文件夹**连带**里面的内容一起删；已用记录开出来的终端**不受影响**（两者互不影响）。
#[derive(Clone, Action, PartialEq, Eq, Deserialize)]
#[action(namespace = alacrterm, no_json)]
pub(crate) struct RemoveEntry {
    pub(crate) path: SessionPath,
}

impl AppRoot {
    /// 注册全局 action 监听器（应在窗口创建后调用一次）。
    ///
    /// 放在这里而不是 `AppRoot::new` 里：那里拿不到已构造好的实体句柄，
    /// 而 `main` 的建窗闭包里正好有 `Entity<AppRoot>`。
    ///
    /// 分发原则：**只改会话列表状态的直接找 [`SessionsState`](crate::sidebar_panel::sessions::SessionsState)
    /// （不需要窗口，连让出一拍都免了）；要开窗口 / 建终端的才落在 `AppRoot` 上**——
    /// 全局监听器只有 `&mut App`，所以那些必须经 [`AppRoot::defer_after_update`]。
    pub(crate) fn register_actions(root: WeakEntity<Self>, cx: &mut App) {
        // 会话列表实体的句柄：只改列表状态的 action（拖放 / 删除）直接找它。
        let Ok(sessions) = root.read_with(cx, |root, _| root.sessions.clone()) else {
            return;
        };

        // —— 新建会话记录：要开对话框（需要窗口），让出一拍再执行 ——
        let dialog_root = root.clone();
        cx.on_action(move |action: &NewSession, cx: &mut App| {
            let folder = action.folder.clone();
            Self::defer_after_update(dialog_root.clone(), cx, move |this, window, cx| {
                this.open_new_session_dialog(folder, window, cx)
            });
        });

        // —— 新建文件夹：同样要开对话框 ——
        let folder_root = root.clone();
        cx.on_action(move |action: &NewFolder, cx: &mut App| {
            let parent = action.parent.clone();
            Self::defer_after_update(folder_root.clone(), cx, move |this, window, cx| {
                this.open_new_folder_dialog(parent, window, cx)
            });
        });

        // —— 按记录开终端：要建终端实体、还要塞进 dock（需要窗口），同样让出一拍 ——
        let open_root = root.clone();
        cx.on_action(move |action: &OpenSession, cx: &mut App| {
            let path = action.path.clone();
            Self::defer_after_update(open_root.clone(), cx, move |this, window, cx| {
                this.open_session_record(&path, window, cx)
            });
        });

        // —— 拖放：把条目挪到别的目录（只改列表状态，不需要窗口）——
        let move_sessions = sessions.clone();
        cx.on_action(move |action: &MoveEntry, cx: &mut App| {
            let from = action.from.clone();
            let into = action.into.clone().unwrap_or_default();
            let _ = move_sessions.update(cx, |state, cx| state.move_entry(&from, &into, cx));
        });

        // —— 删除条目：只改列表状态，不需要窗口 ——
        let remove_sessions = sessions;
        cx.on_action(move |action: &RemoveEntry, cx: &mut App| {
            let path = action.path.clone();
            let _ = remove_sessions.update(cx, |state, cx| state.remove_entry(&path, cx));
        });

        // —— 新建本地终端：不需要对话框，直接开一个本地会话（同样需要窗口）——
        let local_root = root.clone();
        cx.on_action(move |_: &NewLocalTerminal, cx: &mut App| {
            Self::defer_after_update(local_root.clone(), cx, |this, window, cx| {
                this.spawn_terminal(window, cx)
            });
        });

        // —— 打开设置：同样需要窗口（开新窗口），也走 defer ——
        let settings_root = root.clone();
        cx.on_action(move |_: &OpenSettings, cx: &mut App| {
            Self::defer_after_update(settings_root.clone(), cx, |this, window, cx| {
                this.open_settings_window(window, cx)
            });
        });
    }
}
