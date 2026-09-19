//! 应用自定义 Action：菜单项写作 `menu.menu("标签", Box::new(SomeAction))`，点击后由菜单内部
//! `window.dispatch_action(...)` 派发；本模块的 action 一律由 [`App::on_action`] 全局监听器接收。
//!
//! - **会话记录**：[`NewSession`] / [`NewFolder`] / [`OpenSession`] / [`MoveEntry`] / [`RemoveEntry`]；
//! - **终端会话**：[`NewLocalTerminal`]（标签栏 `+` / 欢迎页）；[`OpenSettings`] 开设置窗口。
//!
//! 全局监听器只有 `&mut App`（没有窗口），需要窗口的操作要经
//! [`AppRoot::defer_after_update`]；带数据的 action 只能手写（不进 [`actions!`] 宏）。

use gpui::{Action, App, WeakEntity, actions};
use serde::Deserialize;

use crate::AppRoot;
use crate::sidebar_panel::sessions::SessionPath;

actions!(alacrterm, [NewLocalTerminal, OpenSettings]);

/// 新建一条会话记录（弹建连对话框）。`folder` = 放在哪个文件夹里（`None` = 顶层）。
#[derive(Clone, Action, PartialEq, Eq, Deserialize)]
#[action(namespace = alacrterm, no_json)]
pub(crate) struct NewSession {
    pub(crate) folder: Option<SessionPath>,
}

/// 新建一个文件夹。`parent` = 建在哪个文件夹下（`None` = 顶层）。
#[derive(Clone, Action, PartialEq, Eq, Deserialize)]
#[action(namespace = alacrterm, no_json)]
pub(crate) struct NewFolder {
    pub(crate) parent: Option<SessionPath>,
}

/// 按某条记录新建一个终端（双击该行 / 右键「打开会话」）。
#[derive(Clone, Action, PartialEq, Eq, Deserialize)]
#[action(namespace = alacrterm, no_json)]
pub(crate) struct OpenSession {
    pub(crate) path: SessionPath,
}

/// 把列表里的一个条目挪到另一个目录下（拖动放下）；`into` = 目标文件夹（`None` = 顶层）。
#[derive(Clone, Action, PartialEq, Eq, Deserialize)]
#[action(namespace = alacrterm, no_json)]
pub(crate) struct MoveEntry {
    pub(crate) from: SessionPath,
    pub(crate) into: Option<SessionPath>,
}

/// 删除列表里的一个条目（记录 / 文件夹；文件夹连带里面的内容）。
#[derive(Clone, Action, PartialEq, Eq, Deserialize)]
#[action(namespace = alacrterm, no_json)]
pub(crate) struct RemoveEntry {
    pub(crate) path: SessionPath,
}

impl AppRoot {
    /// 注册全局 action 监听器（窗口创建后调用一次）。
    ///
    /// 只改会话列表状态的直接找 `SessionsState`（不需要窗口）；要开窗口 / 建终端的落在
    /// `AppRoot` 上，经 [`AppRoot::defer_after_update`]。
    pub(crate) fn register_actions(root: WeakEntity<Self>, cx: &mut App) {
        // 列表实体句柄：只改列表状态的 action 直接找它。
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
