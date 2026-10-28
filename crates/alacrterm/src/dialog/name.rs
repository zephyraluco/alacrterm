//! 「输入一个名字」的对话框:文件管理器的**新建文件夹 / 新建文件 / 重命名**共用。
//!
//! 三个动作要的都是「一个名字」,形态完全一样(标题 / 占位符 / 确认按钮三个字不同),
//! 所以只有一个 [`open_name_dialog`],而不是三个对话框。

use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, Entity, IntoElement, ParentElement as _, Styled as _, Window,
    div, px,
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    input::{Input, InputState},
    v_flex,
};

/// 弹一个「输入名字」的对话框。
///
/// `on_ok` 拿到的是**去掉首尾空白**的名字 —— 空白名按不下去(确认按钮是灰的,`on_ok` 也再挡一次)。
/// `initial` 有值就预填(重命名用:旧名字摆在那儿,用户接着改)。
pub(crate) fn open_name_dialog(
    window: &mut Window,
    cx: &mut App,
    title: &'static str,
    placeholder: &'static str,
    confirm: &'static str,
    initial: Option<&str>,
    on_ok: impl Fn(String, &mut App) + 'static,
) {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
    if let Some(initial) = initial {
        input.update(cx, |input, cx| input.set_value(initial, window, cx));
    }
    // ⚠️ `open_dialog` 那个 builder 是 `Fn`(每帧都可能被调用),所以回调得用 `Rc` 共享、
    // 每次进来 clone 一份给 `on_ok`(照 `dialog::folder` 的写法)。
    let on_ok = Rc::new(on_ok);

    window.open_dialog(cx, move |dialog, _window, cx| {
        let valid = !input.read(cx).value().trim().is_empty();
        let footer = DialogFooter::new()
            .child(DialogClose::new().child(Button::new("name-cancel").label("取消")))
            .child(
                DialogAction::new().child(
                    Button::new("name-confirm")
                        .label(confirm)
                        .primary()
                        .disabled(!valid),
                ),
            );

        let on_ok = on_ok.clone();
        let input_for_ok = input.clone();
        dialog
            .title(title)
            .w(px(360.))
            .footer(footer)
            .on_ok(move |_, _, cx| {
                let name = input_for_ok.read(cx).value().trim().to_string();
                if name.is_empty() {
                    return false;
                }
                on_ok(name, cx);
                true
            })
            .child(field(&input, cx))
    });
}

/// 对话框唯一的字段。
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
