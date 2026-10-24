//! 应用自定义 Action：派发后一律由 [`App::on_action`] 全局监听器接收。
//!
//! 键位不写在这里：全部由 [`crate::keymap`] 从 keymap JSON 注册（默认资产
//! `assets/keymaps/default-*.json` + 用户 `config/keymap.json`）。
//! 因此**能被 keymap 引用的 action 必须能从 JSON 构造**：
//!
//! - 零字段 action 用 `actions!` 宏（生成的 unit struct 忽略 JSON 入参）；
//! - 带数据的 action 手写并派生 `Deserialize` + `JsonSchema`，且**不能**加 `no_json`
//!   （加了 `build` 恒失败），keymap 里写成 `["名字", 参数]`。

use gpui::{Action, App, Axis, WeakEntity, actions, px};
use schemars::JsonSchema;
use serde::Deserialize;
use terminal_view::RenderSettings;

use crate::AppRoot;
use crate::pane_layout::Dir;
use crate::sidebar_panel::SidebarView;
use crate::sidebar_panel::sessions::SessionPath;

actions!(
    alacrterm,
    [
        // 会话 / 设置
        NewLocalTerminal,
        OpenSettings,
        // 会话 / 标签
        CloseActiveSession,
        NextSession,
        PrevSession,
        NewSessionDialog,
        // 布局（标签页内部署分屏）
        SplitSessionRight,
        SplitSessionDown,
        FocusPaneLeft,
        FocusPaneRight,
        FocusPaneUp,
        FocusPaneDown,
        // 侧边栏
        ToggleSidebar,
        ShowSessionsView,
        ShowFilesView,
        // 终端操作
        CopyText,
        PasteText,
        ClearTerminal,
        ScrollPageUp,
        ScrollPageDown,
        ScrollToTop,
        ScrollToBottom,
        ScrollLineUp,
        ScrollLineDown,
        ToggleViMode,
        // 字号
        IncreaseFontSize,
        DecreaseFontSize,
        ResetFontSize,
        // 预览窗口（窗口自己接，见 `preview_window.rs` 的 `Render::on_action`）
        SavePreviewFile,
        // 关**当前标签**（最后一个标签关掉 = 关窗口）
        ClosePreviewWindow,
        TogglePreviewWordWrap,
        GotoPreviewLine,
        // 预留：终端内查找（UI 未实现，默认 keymap 里没有绑定）
        SearchTerminal,
    ]
);

/// 跳到第 `index` 个会话（`0` 起）。
///
/// 数据型 action ⇒ keymap 里写成 `["alacrterm::ActivateSession", 0]`。
#[derive(Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema, Action)]
#[action(namespace = alacrterm)]
pub(crate) struct ActivateSession(pub(crate) usize);

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

/// Edit a saved connection without changing any live terminal.
#[derive(Clone, Action, PartialEq, Eq, Deserialize)]
#[action(namespace = alacrterm, no_json)]
pub(crate) struct EditSession {
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

/// 字号上下界（`Ctrl+=` / `Ctrl+-`；设置窗口的数字输入另有自己的范围）。
const MIN_FONT_SIZE: f32 = 6.0;
const MAX_FONT_SIZE: f32 = 48.0;

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

        let edit_root = root.clone();
        cx.on_action(move |action: &EditSession, cx: &mut App| {
            let path = action.path.clone();
            Self::defer_after_update(edit_root.clone(), cx, move |this, window, cx| {
                this.open_edit_session_dialog(&path, window, cx)
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

        let session_dialog_root = root.clone();
        cx.on_action(move |_: &NewSessionDialog, cx: &mut App| {
            Self::defer_after_update(session_dialog_root.clone(), cx, |this, window, cx| {
                this.open_new_session_dialog(None, window, cx)
            });
        });

        let settings_root = root.clone();
        cx.on_action(move |_: &OpenSettings, cx: &mut App| {
            Self::defer_after_update(settings_root.clone(), cx, |this, window, cx| {
                this.open_settings_window(window, cx)
            });
        });

        // 标签 / 布局：都要动窗口（焦点 / dock 布局）。
        let close_root = root.clone();
        cx.on_action(move |_: &CloseActiveSession, cx: &mut App| {
            Self::defer_after_update(close_root.clone(), cx, |this, window, cx| {
                this.close_active_session(window, cx)
            });
        });

        let next_root = root.clone();
        cx.on_action(move |_: &NextSession, cx: &mut App| {
            Self::defer_after_update(next_root.clone(), cx, |this, window, cx| {
                this.step_session(1, window, cx)
            });
        });

        let prev_root = root.clone();
        cx.on_action(move |_: &PrevSession, cx: &mut App| {
            Self::defer_after_update(prev_root.clone(), cx, |this, window, cx| {
                this.step_session(-1, window, cx)
            });
        });

        let activate_root = root.clone();
        cx.on_action(move |action: &ActivateSession, cx: &mut App| {
            let index = action.0;
            Self::defer_after_update(activate_root.clone(), cx, move |this, window, cx| {
                this.activate_session(index, window, cx)
            });
        });

        let split_right_root = root.clone();
        cx.on_action(move |_: &SplitSessionRight, cx: &mut App| {
            Self::defer_after_update(split_right_root.clone(), cx, |this, window, cx| {
                this.split_active_session(Axis::Horizontal, window, cx)
            });
        });

        let split_down_root = root.clone();
        cx.on_action(move |_: &SplitSessionDown, cx: &mut App| {
            Self::defer_after_update(split_down_root.clone(), cx, |this, window, cx| {
                this.split_active_session(Axis::Vertical, window, cx)
            });
        });

        let focus_left_root = root.clone();
        cx.on_action(move |_: &FocusPaneLeft, cx: &mut App| {
            Self::defer_after_update(focus_left_root.clone(), cx, |this, window, cx| {
                this.focus_pane(Dir::Left, window, cx)
            });
        });

        let focus_right_root = root.clone();
        cx.on_action(move |_: &FocusPaneRight, cx: &mut App| {
            Self::defer_after_update(focus_right_root.clone(), cx, |this, window, cx| {
                this.focus_pane(Dir::Right, window, cx)
            });
        });

        let focus_up_root = root.clone();
        cx.on_action(move |_: &FocusPaneUp, cx: &mut App| {
            Self::defer_after_update(focus_up_root.clone(), cx, |this, window, cx| {
                this.focus_pane(Dir::Up, window, cx)
            });
        });

        let focus_down_root = root.clone();
        cx.on_action(move |_: &FocusPaneDown, cx: &mut App| {
            Self::defer_after_update(focus_down_root.clone(), cx, |this, window, cx| {
                this.focus_pane(Dir::Down, window, cx)
            });
        });

        // 侧边栏：不需要窗口。
        let sidebar_toggle = root.clone();
        cx.on_action(move |_: &ToggleSidebar, cx: &mut App| {
            let _ = sidebar_toggle.update(cx, |this, cx| this.toggle_sidebar(cx));
        });

        let sidebar_sessions = root.clone();
        cx.on_action(move |_: &ShowSessionsView, cx: &mut App| {
            let _ = sidebar_sessions
                .update(cx, |this, cx| this.open_sidebar_view(SidebarView::Sessions, cx));
        });

        let sidebar_files = root.clone();
        cx.on_action(move |_: &ShowFilesView, cx: &mut App| {
            let _ = sidebar_files
                .update(cx, |this, cx| this.open_sidebar_view(SidebarView::Files, cx));
        });

        // 终端操作：作用于「当前会话」，不需要窗口。
        let copy_root = root.clone();
        cx.on_action(move |_: &CopyText, cx: &mut App| {
            let _ = copy_root.update(cx, |this, cx| {
                this.with_active_terminal(cx, |view, cx| view.copy_text(cx));
            });
        });

        let paste_root = root.clone();
        cx.on_action(move |_: &PasteText, cx: &mut App| {
            let _ = paste_root.update(cx, |this, cx| {
                this.with_active_terminal(cx, |view, cx| view.paste_text(cx));
            });
        });

        let clear_root = root.clone();
        cx.on_action(move |_: &ClearTerminal, cx: &mut App| {
            let _ = clear_root.update(cx, |this, cx| {
                this.with_active_terminal(cx, |view, cx| view.clear_terminal(cx));
            });
        });

        let scroll_up_root = root.clone();
        cx.on_action(move |_: &ScrollPageUp, cx: &mut App| {
            let _ = scroll_up_root.update(cx, |this, cx| {
                this.with_active_terminal(cx, |view, cx| view.scroll_page_up(cx));
            });
        });

        let scroll_down_root = root.clone();
        cx.on_action(move |_: &ScrollPageDown, cx: &mut App| {
            let _ = scroll_down_root.update(cx, |this, cx| {
                this.with_active_terminal(cx, |view, cx| view.scroll_page_down(cx));
            });
        });

        let scroll_top_root = root.clone();
        cx.on_action(move |_: &ScrollToTop, cx: &mut App| {
            let _ = scroll_top_root.update(cx, |this, cx| {
                this.with_active_terminal(cx, |view, cx| view.scroll_to_top(cx));
            });
        });

        let scroll_bottom_root = root.clone();
        cx.on_action(move |_: &ScrollToBottom, cx: &mut App| {
            let _ = scroll_bottom_root.update(cx, |this, cx| {
                this.with_active_terminal(cx, |view, cx| view.scroll_to_bottom(cx));
            });
        });

        let scroll_line_up_root = root.clone();
        cx.on_action(move |_: &ScrollLineUp, cx: &mut App| {
            let _ = scroll_line_up_root.update(cx, |this, cx| {
                this.with_active_terminal(cx, |view, cx| view.scroll_line_up(cx));
            });
        });

        let scroll_line_down_root = root.clone();
        cx.on_action(move |_: &ScrollLineDown, cx: &mut App| {
            let _ = scroll_line_down_root.update(cx, |this, cx| {
                this.with_active_terminal(cx, |view, cx| view.scroll_line_down(cx));
            });
        });

        let vi_root = root.clone();
        cx.on_action(move |_: &ToggleViMode, cx: &mut App| {
            let _ = vi_root.update(cx, |this, cx| {
                this.with_active_terminal(cx, |view, cx| view.toggle_vi_mode(cx));
            });
        });

        // 字号：与设置窗口同一条路径（落盘 + 改全局 + 推给会话）。
        let font_up_root = root.clone();
        cx.on_action(move |_: &IncreaseFontSize, cx: &mut App| {
            crate::settings_window::update_render(&font_up_root, cx, |settings| {
                settings.font_size =
                    px((settings.font_size.as_f32() + 1.0).min(MAX_FONT_SIZE));
            });
        });

        let font_down_root = root.clone();
        cx.on_action(move |_: &DecreaseFontSize, cx: &mut App| {
            crate::settings_window::update_render(&font_down_root, cx, |settings| {
                settings.font_size =
                    px((settings.font_size.as_f32() - 1.0).max(MIN_FONT_SIZE));
            });
        });

        let font_reset_root = root.clone();
        cx.on_action(move |_: &ResetFontSize, cx: &mut App| {
            crate::settings_window::update_render(&font_reset_root, cx, |settings| {
                settings.font_size = RenderSettings::default().font_size;
            });
        });

        // 预留动作：注册了但默认不绑定，只为让用户能在 keymap 里占位。
        cx.on_action(move |_: &SearchTerminal, _cx: &mut App| {
            eprintln!("[keymap] 终端内查找尚未实现（alacrterm::SearchTerminal）");
        });
    }
}
