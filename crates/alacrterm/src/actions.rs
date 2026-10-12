//! 应用自定义 Action：派发后一律由 [`App::on_action`] 全局监听器接收。

use gpui::{Action, App, WeakEntity, actions};
use serde::Deserialize;

use crate::AppRoot;
use crate::sidebar_panel::sessions::SessionPath;

actions!(alacrterm, [NewLocalTerminal, OpenSettings]);

/// 新建一条会话记录。
#[derive(Clone, Action, PartialEq, Eq, Deserialize)]
#[action(namespace = alacrterm, no_json)]
pub(crate) struct NewSession {
    pub(crate) folder: Option<SessionPath>,
}

/// 新建一个文件夹。
#[derive(Clone, Action, PartialEq, Eq, Deserialize)]
#[action(namespace = alacrterm, no_json)]
pub(crate) struct NewFolder {
    pub(crate) parent: Option<SessionPath>,
}

/// 按某条记录新建一个终端。
#[derive(Clone, Action, PartialEq, Eq, Deserialize)]
#[action(namespace = alacrterm, no_json)]
pub(crate) struct OpenSession {
    pub(crate) path: SessionPath,
}

/// 把列表里的一个条目挪到另一个目录下。
#[derive(Clone, Action, PartialEq, Eq, Deserialize)]
#[action(namespace = alacrterm, no_json)]
pub(crate) struct MoveEntry {
    pub(crate) from: SessionPath,
    pub(crate) into: Option<SessionPath>,
}

/// 删除列表里的一个条目。
#[derive(Clone, Action, PartialEq, Eq, Deserialize)]
#[action(namespace = alacrterm, no_json)]
pub(crate) struct RemoveEntry {
    pub(crate) path: SessionPath,
}

impl AppRoot {
    /// 注册全局 action 监听器。
    pub(crate) fn register_actions(root: WeakEntity<Self>, cx: &mut App) {
        let Ok(sessions) = root.read_with(cx, |root, _| root.sessions.clone()) else {
            return;
        };

        let dialog_root = root.clone();
        cx.on_action(move |action: &NewSession, cx: &mut App| {
            let folder = action.folder.clone();
            Self::defer_after_update(dialog_root.clone(), cx, move |this, window, cx| {
                this.open_new_session_dialog(folder, window, cx)
            });
        });

        let folder_root = root.clone();
        cx.on_action(move |action: &NewFolder, cx: &mut App| {
            let parent = action.parent.clone();
            Self::defer_after_update(folder_root.clone(), cx, move |this, window, cx| {
                this.open_new_folder_dialog(parent, window, cx)
            });
        });

        let open_root = root.clone();
        cx.on_action(move |action: &OpenSession, cx: &mut App| {
            let path = action.path.clone();
            Self::defer_after_update(open_root.clone(), cx, move |this, window, cx| {
                this.open_session_record(&path, window, cx)
            });
        });

        let move_sessions = sessions.clone();
        cx.on_action(move |action: &MoveEntry, cx: &mut App| {
            let from = action.from.clone();
            let into = action.into.clone().unwrap_or_default();
            let _ = move_sessions.update(cx, |state, cx| state.move_entry(&from, &into, cx));
        });

        let remove_sessions = sessions;
        cx.on_action(move |action: &RemoveEntry, cx: &mut App| {
            let path = action.path.clone();
            let _ = remove_sessions.update(cx, |state, cx| state.remove_entry(&path, cx));
        });

        let local_root = root.clone();
        cx.on_action(move |_: &NewLocalTerminal, cx: &mut App| {
            Self::defer_after_update(local_root.clone(), cx, |this, window, cx| {
                this.spawn_terminal(window, cx)
            });
        });

        let settings_root = root.clone();
        cx.on_action(move |_: &OpenSettings, cx: &mut App| {
            Self::defer_after_update(settings_root.clone(), cx, |this, window, cx| {
                this.open_settings_window(window, cx)
            });
        });
    }
}
