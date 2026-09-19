//! 「新建文件夹」对话框：给侧边栏的会话列表建一个文件夹。
//!
//! 由侧边栏「会话」栏底部状态栏**左下角的文件夹图标**触发（`NewFolder` action →
//! [`AppRoot::open_new_folder_dialog`]，`parent` 为 `None` ⇒ 建在顶层），
//! 文件夹行的右键菜单「新建子文件夹」走同一个入口，只是带上 `parent`。
//!
//! 与 [`crate::dialog`] 里的其它对话框一样：**只改列表结构，不碰任何终端实例**。
//! 文件夹名不能为空（「创建」按钮的禁用态与 `on_ok` 里的兜底校验都读它）。

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Styled as _,
    Window, div, px,
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    input::{Input, InputState},
    v_flex,
};

use crate::sidebar_panel::sessions::SessionPath;
use crate::AppRoot;

/// 文件夹表单：一个名字输入框（实体长期存活，避免对话框每帧重建时丢掉输入）。
#[derive(Clone)]
pub(crate) struct FolderForm {
    name: Entity<InputState>,
}

impl FolderForm {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<AppRoot>) -> Self {
        Self {
            name: cx.new(|cx| InputState::new(window, cx).placeholder("例如：生产环境")),
        }
    }

    /// 去掉首尾空白后的文件夹名。
    fn trimmed_name(&self, cx: &App) -> String {
        self.name.read(cx).value().trim().to_string()
    }
}

impl AppRoot {
    /// 弹出「新建文件夹」对话框；`parent` = 建在哪个文件夹下（`None` = 顶层）。
    pub(crate) fn open_new_folder_dialog(
        &mut self,
        parent: Option<SessionPath>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let form = FolderForm::new(window, cx);
        // 加文件夹写的是**会话列表实体**（`SessionsState`）的状态，直接把它的句柄带进对话框。
        let sessions = self.sessions.clone();

        window.open_dialog(cx, move |dialog, _window, cx| {
            let form_for_ok = form.clone();
            let sessions_for_ok = sessions.clone();
            let parent_for_ok = parent.clone();
            // 空名字不允许创建：驱动「创建」的禁用态（`on_ok` 里再兜一次）。
            let valid = !form.trimmed_name(cx).is_empty();

            let footer = DialogFooter::new()
                .child(DialogClose::new().child(Button::new("folder-cancel").label("取消")))
                .child(DialogAction::new().child(
                    Button::new("folder-confirm")
                        .label("创建")
                        .primary()
                        .disabled(!valid),
                ));

            dialog
                .title("新建文件夹")
                .w(px(360.))
                .footer(footer)
                .on_ok(move |_, _, cx| {
                    let name = form_for_ok.trimmed_name(cx);
                    if name.is_empty() {
                        return false;
                    }
                    let parent = parent_for_ok.clone();
                    // 列表状态在 `SessionsState` 里 ⇒ 直接写它（不需要窗口，无需 defer）。
                    sessions_for_ok.update(cx, |state, cx| {
                        state.add_folder(name.into(), parent, cx)
                    });
                    true
                })
                .child(field(&form.name, cx))
        });
    }
}

/// 唯一那个字段：标题 + 输入框。
fn field(input: &Entity<InputState>, cx: &App) -> AnyElement {
    v_flex()
        .gap_1()
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("名称"),
        )
        .child(Input::new(input))
        .into_any_element()
}
