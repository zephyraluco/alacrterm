//! 终端容器关闭后的**欢迎页**（中间列的默认背景板）：内容居中、列宽固定、列内元素左对齐，
//! 「圆角方块 logo + 标题 + 副标题」下面接一节「开始使用」（分节标题 + 分隔线 + 操作行）；
//! 背景用主题 `background`（与终端区同色）。
//!
//! 所有标签页都关掉后中间列改渲染本页（见 [`crate::AppRoot::render`]）。

use gpui::{
    AnyElement, Context, CursorStyle, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, StatefulInteractiveElement as _, Styled as _, Window, div, px, svg,
};
// `h_flex` = `div().flex().flex_row().items_center()` 的速记（单写 `.flex_row()` 不会设置 `display: flex`）。
use gpui_kit::base::h_flex;
use gpui_kit::component::{ActiveTheme as _, IconNamed as _, v_flex};

use crate::AppRoot;
use crate::actions::NewLocalTerminal;
use crate::assets::IconName;

/// 欢迎页内容列宽：整列居中，列内元素左对齐（同 zed 欢迎页）。
const WELCOME_CONTENT_WIDTH: Pixels = px(420.);
/// 品牌区：logo 方块边长 / 方块内图标尺寸。
const LOGO_BOX_SIZE: Pixels = px(52.);
const LOGO_ICON_SIZE: Pixels = px(28.);
/// 操作行：行高 / 行内图标尺寸。
const ACTION_ROW_HEIGHT: Pixels = px(26.);
const ACTION_ICON_SIZE: Pixels = px(14.);
/// 「新建终端」的快捷键提示（跟随平台：macOS 用 ⌘，其余用 Ctrl）。
const NEW_TERMINAL_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "⌘ N"
} else {
    "Ctrl N"
};

impl AppRoot {
    /// 欢迎页：终端容器关闭（所有标签都关掉）后中间列的默认内容。
    pub(crate) fn render_welcome(&self, cx: &mut Context<Self>) -> AnyElement {
        let background = cx.theme().background;
        let foreground = cx.theme().foreground;
        let muted = cx.theme().muted;
        let muted_foreground = cx.theme().muted_foreground;
        let border = cx.theme().border;

        v_flex()
            .size_full()
            // 整块居中；内边距加在整页外层，内容列的宽度上限仍是完整的 420px。
            .items_center()
            .justify_center()
            .px(px(24.))
            .bg(background)
            .child(
                v_flex()
                    .w_full()
                    .max_w(WELCOME_CONTENT_WIDTH)
                    .gap(px(22.))
                    // —— 品牌区：圆角方块 logo + 标题 + 副标题 ——
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
                                            // `svg()` 不继承父元素颜色。
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
                    // —— 开始使用：分节标题 + 分隔线 + 操作行 ——
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

    /// 欢迎页里的一行操作：左端图标 + 名称，右端快捷键；整行可点、悬停提亮。
    ///
    /// `id` 必须全局唯一（gpui 状态化元素的要求），由调用方给出。
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

        // 快捷键可有可无（没有绑定的操作不显示提示）。
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
