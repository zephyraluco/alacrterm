//! 自绘终端标签栏：**不使用** gpui-kit 的 `TabBar` / `Tab`（及 `Button` / `Icon`）组件。
//!
//! 结构与交互参考 zed：
//! - `crates/ui/src/components/tab.rs`（`Tab`：`container_height` / `content_height`、
//!   选中用 `tab_active` / `tab_active_foreground`，未选中用 `tab_foreground`、
//!   选中标签用「底部 1px 内边距」盖住栏底分隔线，即 `pb_px()`）；
//! - `crates/ui/src/components/tab_bar.rs`（`TabBar`：`tab_bar` 底色 + 绝对定位的
//!   底边线 + `overflow_x_scroll()` / `track_scroll()` 的标签滚动区 + 右端固定按钮区）；
//! - `crates/terminal_view/src/terminal_view.rs` 的 `tab_content`（标签 = 图标 + 标题，
//!   图标取终端的 muted 色，标题单行截断）。
//!
//! 本模块的取舍：
//! - 只用 gpui 原语绘制（`div` / `svg`）：图标不套 `Icon` 组件，而是
//!   `svg().path(IconName::X.path())` 配 `.text_color(...)` 着色；关闭按钮是一个
//!   手绘的圆角 `div` + `svg`，不是 `Button`。
//! - **关闭按钮只在鼠标悬停该标签、或它是选中标签时才渲染**（zed 的
//!   `show_close_button: hover`）。gpui-pre 没有 `visible_on_hover` / `invisible`，
//!   这里用 `AppRoot::hovered_tab` 记录悬停下标后「索性不渲染」——比
//!   `opacity(0.)` 干净：后者会留下看不见但可点击的热区。
//! - 中键点击标签也能关闭（zed 的 `on_mouse_up(MouseButton::Middle, ...)`）。
//! - 标签多了横向滚动：`overflow_x_scroll()` + `track_scroll()`，gpui 会把垂直滚轮
//!   自动映射为横向滚动（zed 的 `TabBar` 同样只依赖这一点，没有额外的滚轮处理）；
//!   激活标签时由 `ScrollHandle::scroll_to_item` 滚进可视区。
//! - 右端固定区放一枚「+」（派发 `NewTerminal`，与侧边栏右键菜单同一入口），
//!   与标签区之间用一条竖线隔开——对应 zed `TabBar` 的 `end_children`。

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, CursorStyle, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, Pixels, StatefulInteractiveElement as _, Styled as _, div, px, svg,
};
// `h_flex` 来自 gpui-base（即 `gpui_kit::base`，gpui 原语层）：
// 它就是 `div().flex().flex_row().items_center()` 的速记，不是组件。
// （注意：单独 `.flex_row()` 并不会设置 `display: flex`，构造 flex 容器必须走 `flex()`。）
use gpui_kit::base::h_flex;
use gpui_kit::component::{ActiveTheme as _, IconNamed as _};

use crate::AppRoot;
use crate::Session;
use crate::actions::NewTerminal;
use crate::assets::IconName;

/// 标签栏高度（zed 的 `Tab::container_height()` 取 Base32，即 32px）。
const TAB_BAR_HEIGHT: Pixels = px(32.);
/// 标签内容区高度 = 栏高 − 1px：选中标签靠「底部留 1px」盖住栏底那条分隔线。
const TAB_CONTENT_HEIGHT: Pixels = px(31.);
/// 标签内左右内边距。
const TAB_PADDING_X: Pixels = px(8.);
/// 图标 / 标题 / 关闭按钮之间的间距。
const TAB_GAP: Pixels = px(6.);
/// 标签宽度（固定）。宽度可预测、好扫视；与 zed 的 `SystemWindowTabs`（宽度不随标题变化）
/// 同思路。注意不能用「内容撑宽 + `max_w` 封顶」：那样标题会溢出标签背景（内容行
/// 宽度仍是内容的宽度），而给内容行 `w_full` + 内容撑宽又会互相抵消、把标签压扁。
const TAB_WIDTH: Pixels = px(200.);
/// 标签内图标尺寸、关闭图标尺寸、关闭按钮热区尺寸。
const TAB_ICON_SIZE: Pixels = px(12.);
const TAB_CLOSE_ICON_SIZE: Pixels = px(10.);
const TAB_CLOSE_BUTTON_SIZE: Pixels = px(16.);
/// 右端「+」按钮尺寸。
const NEW_TAB_BUTTON_SIZE: Pixels = px(20.);

impl AppRoot {
    /// 终端标签栏（自绘，见模块文档）。
    pub(crate) fn render_terminal_tab_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;

        // 先物化成 Vec：`.children(迭代器)` 会把闭包连同它对 `cx` 的可变借用一起
        // 带进元素树，后面再用 `cx` 渲染右端按钮就会冲突。
        let tabs: Vec<AnyElement> = self
            .terminals
            .iter()
            .enumerate()
            .map(|(ix, session)| self.render_tab(ix, session, cx))
            .collect();

        h_flex()
            .w_full()
            .h(TAB_BAR_HEIGHT)
            .bg(cx.theme().tab_bar)
            // 标签滚动区：底边线用绝对定位铺满，标签浮在它上面（选中标签盖住它）。
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            .border_b_1()
                            .border_color(border),
                    )
                    .child(
                        h_flex()
                            .id("terminal-tab-strip")
                            .w_full()
                            .h_full()
                            .overflow_x_scroll()
                            .track_scroll(&self.tab_scroll_handle)
                            .children(tabs),
                    ),
            )
            // 右端固定区（对应 zed `TabBar` 的 `end_children`）：新建终端。
            .child(
                h_flex()
                    .flex_none()
                    .h_full()
                    .px_1()
                    .border_l_1()
                    .border_b_1()
                    .border_color(border)
                    .child(self.render_new_tab_button(cx)),
            )
            .into_any_element()
    }

    /// 单个标签：图标 + 标题（单行截断）+ 关闭按钮。
    fn render_tab(&self, ix: usize, session: &Session, cx: &mut Context<Self>) -> AnyElement {
        let selected = ix == self.active;
        // 关闭按钮：选中标签常驻，其余只在鼠标悬停它时出现（见模块文档）。
        let show_close = selected || self.hovered_tab == Some(ix);

        // 配色对齐 zed：选中 = `tab_active` / `tab_active_foreground`；
        // 未选中 = 透明底 + `tab_foreground`，悬停时用 `muted` 提亮。
        let (background, foreground) = if selected {
            (cx.theme().tab_active, cx.theme().tab_active_foreground)
        } else {
            (gpui::transparent_black(), cx.theme().tab_foreground)
        };
        let hover_background = cx.theme().muted;
        let icon_color = cx.theme().muted_foreground;

        h_flex()
            .id(("terminal-tab", ix))
            .flex_none()
            .w(TAB_WIDTH)
            .items_center()
            .h(TAB_BAR_HEIGHT)
            .bg(background)
            .text_color(foreground)
            .cursor(CursorStyle::PointingHand)
            // 选中标签底部留 1px：让栏底那条分隔线在它下面露不出来，与下方终端连成一体。
            .when(selected, |this| this.pb(px(1.)))
            .when(!selected, |this| {
                this.hover(move |style| style.bg(hover_background))
            })
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                let next = hovered.then_some(ix);
                if this.hovered_tab != next {
                    this.hovered_tab = next;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| this.set_active_tab(ix, cx)))
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(move |this, _, _, cx| this.close_terminal(ix, cx)),
            )
            .child(
                h_flex()
                    .w_full()
                    .h(TAB_CONTENT_HEIGHT)
                    .px(TAB_PADDING_X)
                    .gap(TAB_GAP)
                    .child(
                        svg()
                            .path(IconName::SquareTerminal.path())
                            .flex_none()
                            .w(TAB_ICON_SIZE)
                            .h(TAB_ICON_SIZE)
                            .text_color(icon_color),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(session.title(cx)),
                    )
                    // 关闭按钮的位置**始终**占位（空槽也是一个固定尺寸的 div）：
                    // 它只在悬停 / 选中时出现，若槽位跟着伸缩，标题会跟着抖动。
                    .child(self.render_tab_close_slot(ix, show_close, cx))
            )
            .into_any_element()
    }

    /// 标签上的关闭按钮槽位：固定尺寸，仅在 `show_close` 时里面放一个手绘的 `svg` 叉号。
    fn render_tab_close_slot(
        &self,
        ix: usize,
        show_close: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // ⚠️ 必须是真正设置 `display: flex` 的容器：只写 `.flex_row()` 只是设了
        // `flex-direction`，盒子仍是块级，`items_center` / `justify_center` 全部失效，
        // 里面的叉号会贴在槽位左上角（这就是「x 不在按钮正中」的原因）。
        // `h_flex()` = `flex() + flex_row() + items_center()`，是 gpui 原语不是组件。
        let slot = h_flex()
            .flex_none()
            .justify_center()
            .size(TAB_CLOSE_BUTTON_SIZE);

        if !show_close {
            return slot.into_any_element();
        }

        slot.id(("terminal-tab-close", ix))
            .rounded_sm()
            .cursor(CursorStyle::PointingHand)
            .hover(|style| style.bg(cx.theme().muted))
            .on_click(cx.listener(move |this, _, _, cx| {
                // 别让这次点击冒泡到标签本身——那会再激活一次这个（即将被移除的）下标。
                cx.stop_propagation();
                this.close_terminal(ix, cx);
            }))
            .child(
                svg()
                    .path(IconName::Close.path())
                    .w(TAB_CLOSE_ICON_SIZE)
                    .h(TAB_CLOSE_ICON_SIZE)
                    // ⚠️ `svg()` 不会从父元素继承 `text_color`，不显式着色就按默认色画，
                    // 深色背景上等于看不见（终端图标与「+」同样必须自己设色）。
                    .text_color(cx.theme().muted_foreground),
            )
            .into_any_element()
    }

    /// 右端「+」：新建终端（派发 `NewTerminal`，与侧边栏会话条目的右键菜单同一入口）。
    fn render_new_tab_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted;
        let icon_color = cx.theme().muted_foreground;

        h_flex()
            .id("terminal-new-tab")
            .justify_center()
            .size(NEW_TAB_BUTTON_SIZE)
            .rounded_sm()
            .cursor(CursorStyle::PointingHand)
            .hover(move |style| style.bg(muted))
            .on_click(|_, window, cx| {
                window.dispatch_action(Box::new(NewTerminal), cx);
            })
            .child(
                svg()
                    .path(IconName::Plus.path())
                    .w(px(12.))
                    .h(px(12.))
                    // 同叉号：`svg()` 不继承父元素颜色，必须显式着色。
                    .text_color(icon_color),
            )
            .into_any_element()
    }
}
