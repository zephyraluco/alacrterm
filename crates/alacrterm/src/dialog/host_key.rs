//! 「未知主机密钥」确认对话框：首次连接主机时让用户核对指纹。

use gpui::{
    App, Context, IntoElement, ParentElement as _, Styled as _, WeakEntity, Window, div, px,
};
use gpui_kit::component::{
    ActiveTheme as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    h_flex, v_flex,
};
use terminal::{HostKeyDecision, HostKeyPrompt};
use terminal_view::HostKeyPromptHandler;

use crate::AppRoot;

/// 造主机密钥确认回调。
pub(crate) fn host_key_prompt_handler(root: WeakEntity<AppRoot>) -> HostKeyPromptHandler {
    std::sync::Arc::new(move |prompt: &HostKeyPrompt, cx: &mut App| {
        let prompt = prompt.clone();
        let root = root.clone();
        AppRoot::defer_after_update(root, cx, move |root, window, cx| {
            root.open_host_key_dialog(prompt, window, cx);
        });
    })
}

impl AppRoot {
    /// 弹「信任这台主机吗」对话框。
    fn open_host_key_dialog(
        &mut self,
        prompt: HostKeyPrompt,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.open_dialog(cx, move |dialog, _window, cx| {
            let accept = prompt.clone();
            let reject = prompt.clone();
            let body = host_key_body(&prompt, cx);

            let footer = DialogFooter::new()
                .child(DialogClose::new().child(Button::new("reject").label("不信任")))
                .child(DialogAction::new().child(Button::new("accept").label("信任并继续").primary()));

            dialog
                .title("未知主机密钥")
                .w(px(460.))
                .footer(footer)
                .on_ok(move |_, _, _| {
                    accept.respond(HostKeyDecision::Accept);
                    true
                })
                .on_cancel(move |_, _, _| {
                    reject.respond(HostKeyDecision::Reject);
                    true
                })
                .on_close({
                    let reject = prompt.clone();
                    move |_, _, _| reject.respond(HostKeyDecision::Reject)
                })
                .child(body)
        });
    }
}

/// 对话框正文。
fn host_key_body(prompt: &HostKeyPrompt, cx: &App) -> gpui::AnyElement {
    let muted = cx.theme().muted_foreground;
    let line = |label: &'static str, value: String| {
        h_flex()
            .gap_2()
            .items_center()
            .child(div().flex_shrink_0().text_color(muted).child(label))
            .child(value)
            .into_any_element()
    };

    v_flex()
        .gap_2()
        .child(line("主机", prompt.endpoint()))
        .child(line("密钥类型", prompt.key_type.clone()))
        .child(
            v_flex()
                .gap_1()
                .child(div().text_color(muted).child("指纹"))
                .child(div().child(prompt.fingerprint.clone())),
        )
        .child(
            div()
                .pt_1()
                .text_xs()
                .text_color(muted)
                .child("请与服务器管理员核对指纹。信任后它会记进 known_hosts，下次不再询问。"),
        )
        .into_any_element()
}
