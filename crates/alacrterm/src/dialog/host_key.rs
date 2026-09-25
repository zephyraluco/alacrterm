//! 「未知主机密钥」确认对话框：首次连接一台主机时让用户核对指纹再决定信任。
//!
//! 触发链：SSH 握手 → `ssh::HostKeyPrompt` → `terminal::Event::HostKeyPrompt` →
//! [`terminal_view::HostKeyPromptHandler`] → 本模块。用户的选择经
//! [`HostKeyPrompt::respond`] 回给**正在等待的握手线程**（所以这里必须尽快弹窗；
//! 超时不答则放弃连接，见 `ssh::HOST_KEY_TIMEOUT`）。
//!
//! 只处理「没见过的主机」；**密钥变了**（与 `known_hosts` 里记录的不一致）一律直接拒绝，
//! 不走这里 —— 那种情况要用户自己清理 `known_hosts`（见 `ssh::SshError::HostKeyChanged`）。

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

/// 造一个主机密钥确认回调（传给 [`terminal_view::TerminalView::new_ssh`]）。
///
/// 回调发生在终端事件的处理过程中（此时主窗口正在更新栈上），所以弹窗要经
/// [`AppRoot::defer_after_update`] 推迟一拍 —— 直接开窗会拿不到窗口。
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
    /// 弹「信任这台主机吗」对话框，并把用户的选择回给握手线程。
    ///
    /// 取消 / 关闭 / 点 X 都算**不信任**（`respond` 只认第一次回答，重复调用无副作用）。
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

            // 页脚自拼：`Dialog::render` 不自动生成确定 / 取消按钮（同 `connection`）。
            // 「信任」是主按钮，「不信任」走 `DialogClose`（派发 Cancel）。
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

/// 对话框正文：端点、密钥算法，以及**独占一行**的指纹（长串横排会被对话框裁掉）。
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
