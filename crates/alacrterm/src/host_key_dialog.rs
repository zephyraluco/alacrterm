//! SSH 主机密钥确认对话框：首次连接 / 密钥变更时问用户「是 / 否」。
//!
//! 触发链：`ssh::ClientHandler::check_server_key` 需要用户拍板 → 经
//! `TerminalBackendEvent::HostKeyPrompt` 送进 `Terminal` 实体 → 转成
//! `terminal_view::Event::HostKeyPrompt` → 由应用层提供的处理器
//! （[`crate::terminal_panel`] 里的 `host_key_prompt_handler`）转给根视图 →
//! 在**主窗口**上弹出本对话框。用户点的「信任并继续 / 取消」就是
//! `HostKeyPrompt::respond` 的答案，SSH 线程拿到后继续握手或放弃连接。
//!
//! 为什么走「应用层 + 窗口级对话框」而不在终端视图里画浮层：连接是在后台完成的，
//! 发起连接的会话未必是当前标签页——浮层会没人看到，而对话框是整个窗口共享的。
//!
//! 不回答也不会卡住：对话框被关掉（`on_close`）→ 拒绝；窗口/应用退出导致请求被
//! 丢弃 → 等待方读到通道关闭 → 拒绝；超时（见 `ssh::HOST_KEY_TIMEOUT`）→ 拒绝。

use gpui::{
    AnyElement, App, Context, Hsla, IntoElement, ParentElement as _, Styled as _, Window, div, px,
};
use gpui_kit::component::{
    ActiveTheme as _, WindowExt as _,
    dialog::DialogButtonProps,
    v_flex,
};
use terminal::{HostKeyDecision, HostKeyPrompt, HostKeyState};

use crate::AppRoot;

impl AppRoot {
    /// 在主窗口上弹出「是否信任这台主机」的对话框。
    ///
    /// 由 [`crate::terminal_panel`] 经 [`AppRoot::defer_after_update`] 调用：
    /// 事件回调里只有 `&mut App`（拿不到窗口），让出一拍后窗口才回到可用状态。
    pub(crate) fn show_host_key_dialog(
        &mut self,
        prompt: HostKeyPrompt,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = match prompt.state() {
            HostKeyState::Unknown => "首次连接这台主机",
            HostKeyState::Changed { .. } => "主机密钥已变更",
        };

        window.open_alert_dialog(cx, move |alert, _window, cx| {
            // 对话框的构建闭包是 `Fn`（每次渲染都会被调用），所以在这里克隆；
            // `respond` 只认第一次调用，多点几次也不会把答案搞乱。
            let trust = prompt.clone();
            let cancel = prompt.clone();
            let close = prompt.clone();

            alert
                .confirm()
                .width(px(560.))
                .title(title)
                .description(body(&prompt, cx))
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("信任并继续")
                        .cancel_text("取消")
                        // `button_props` 会整体替掉 `.confirm()` 设的 `show_cancel`，
                        // 不显式打开就没有「取消」（用户只能被迫信任）。
                        .show_cancel(true),
                )
                .on_ok(move |_, _, _| {
                    trust.respond(HostKeyDecision::Trust);
                    true
                })
                .on_cancel(move |_, _, _| {
                    cancel.respond(HostKeyDecision::Reject);
                    true
                })
                // `on_ok` / `on_cancel` 之后还会调一次 `on_close`（确认按钮、取消、
                // Esc、关闭按钮都走这里）：答案通常已经发出去了，这条是兜底。
                .on_close(move |_, _, _| close.respond(HostKeyDecision::Reject))
        });
    }
}

/// 对话框正文：端点、指纹，以及「为什么问你」。
fn body(prompt: &HostKeyPrompt, cx: &App) -> AnyElement {
    let mut content = v_flex()
        .gap_2()
        .child(field("主机", &prompt.endpoint(), cx))
        .child(field(
            &format!("{} 指纹", prompt.key_type()),
            prompt.fingerprint(),
            cx,
        ));

    match prompt.state() {
        HostKeyState::Unknown => {
            content = content.child(note(
                "这是第一次连接这台主机。请与管理员提供（或上次记录）的指纹核对：\
                 确认后会把该密钥写入 known_hosts，之后密钥不一致将直接拒绝连接。",
                cx.theme().muted_foreground,
            ));
        }
        HostKeyState::Changed { known } => {
            for recorded in known {
                content = content.child(field("已记录", recorded, cx));
            }
            content = content.child(note(
                "⚠️ 服务器上报的密钥与 known_hosts 里的记录不一致：可能是主机重装过，\
                 也可能有人在中间冒充它。确认后会删除旧记录并写入新密钥——\
                 请务必先用可信渠道核对指纹。",
                cx.theme().warning,
            ));
        }
    }

    content.into_any_element()
}

/// 「标签 / 值」两行：指纹有近 50 个字符，和标签横排会顶出对话框被裁掉。
fn field(label: &str, value: &str, cx: &App) -> AnyElement {
    v_flex()
        .gap_1()
        .text_sm()
        .child(
            div()
                .text_color(cx.theme().muted_foreground)
                .child(label.to_string()),
        )
        .child(
            div()
                .text_color(cx.theme().foreground)
                .child(value.to_string()),
        )
        .into_any_element()
}

/// 说明段落（指定颜色的小字）。
fn note(text: &str, color: Hsla) -> AnyElement {
    div()
        .text_sm()
        .text_color(color)
        .child(text.to_string())
        .into_any_element()
}
