//! 所有终端关闭后中间列显示的**欢迎页**。

use gpui::{
    AnyElement, Context, CursorStyle, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, StatefulInteractiveElement as _, Styled as _, Window, div, px, svg,
};
use gpui_kit::base::h_flex;
use gpui_kit::component::{ActiveTheme as _, IconNamed as _, v_flex};

use crate::AppRoot;
use crate::actions::NewLocalTerminal;
use crate::assets::IconName;

const WELCOME_CONTENT_WIDTH: Pixels = px(420.);
const LOGO_BOX_SIZE: Pixels = px(52.);
const LOGO_ICON_SIZE: Pixels = px(28.);
const ACTION_ROW_HEIGHT: Pixels = px(26.);
const ACTION_ICON_SIZE: Pixels = px(14.);
/// 「新建终端」的快捷键提示（与 `assets/keymaps/default-*.json` 里的绑定一致）。
const NEW_TERMINAL_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "⌘ ⇧ N"
} else {
    "Ctrl Shift N"
};

impl AppRoot {
    pub(crate) fn render_welcome(&self, cx: &mut Context<Self>) -> AnyElement {
        let background = cx.theme().background;
        let foreground = cx.theme().foreground;
        let muted = cx.theme().muted;
        let muted_foreground = cx.theme().muted_foreground;
        let border = cx.theme().border;

        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .px(px(24.))
            .bg(background)
            .child(
                v_flex()
                    .w_full()
                    .max_w(WELCOME_CONTENT_WIDTH)
                    .gap(px(22.))
                    .child(
                        h_flex()
                            .gap(px(14.))
                            .child(
                                h_flex()
                                    .flex_none()
                                    .justify_center()
                                    .size(LOGO_BOX_SIZE)
                                    .rounded_lg()
                                    .bg(muted)
                                    .child(
                                        svg()
                                            .path(IconName::SquareTerminal.path())
                                            .w(LOGO_ICON_SIZE)
                                            .h(LOGO_ICON_SIZE)
                                            .text_color(foreground),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .gap(px(2.))
                                    .child(
                                        div()
                                            .text_xl()
                                            .text_color(foreground)
                                            .child("Welcome back to Alacrterm"),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .italic()
                                            .text_color(muted_foreground)
                                            .child("The terminal for what's next"),
                                    ),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted_foreground)
                                    .child("GET STARTED"),
                            )
                            .child(div().h(px(1.)).w_full().bg(border))
                            .child(
                                v_flex()
                                    .gap(px(2.))
                                    .child(self.welcome_action(
                                        "welcome-new-terminal",
                                        IconName::Plus,
                                        "新建终端",
                                        Some(NEW_TERMINAL_SHORTCUT),
                                        cx,
                                        |_, window, cx| {
                                            window.dispatch_action(Box::new(NewLocalTerminal), cx)
                                        },
                                    ))
                                    .child(self.welcome_action(
                                        "welcome-open-settings",
                                        IconName::Settings,
                                        "打开设置",
                                        None,
                                        cx,
                                        |this, window, cx| this.open_settings_window(window, cx),
                                    )),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// 一行操作项：图标 + 名称 + 可选快捷键。
    fn welcome_action(
        &self,
        id: &'static str,
        icon: IconName,
        label: &'static str,
        shortcut: Option<&'static str>,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let foreground = cx.theme().foreground;
        let muted = cx.theme().muted;
        let muted_foreground = cx.theme().muted_foreground;

        let row = h_flex()
            .id(id)
            .h(ACTION_ROW_HEIGHT)
            .px(px(6.))
            .gap(px(10.))
            .rounded_sm()
            .cursor(CursorStyle::PointingHand)
            .hover(move |style| style.bg(muted))
            .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
            .child(
                svg()
                    .path(icon.path())
                    .flex_none()
                    .w(ACTION_ICON_SIZE)
                    .h(ACTION_ICON_SIZE)
                    .text_color(muted_foreground),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_sm()
                    .text_color(foreground)
                    .child(label),
            );

        match shortcut {
            Some(shortcut) => row
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(muted_foreground)
                        .child(shortcut),
                )
                .into_any_element(),
            None => row.into_any_element(),
        }
    }
}
