//! 侧边栏顶部的面板标题。

use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, Styled as _, div,
};
use gpui_kit::component::{ActiveTheme as _, h_flex};

use super::shared::ellipsis_label;
use super::{PANEL_HEADER_HEIGHT, Sidebar, SidebarView};

impl Sidebar {
    /// 本栏顶部的标题栏。
    pub(super) fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let active = self.active_view(cx);

        h_flex()
            .id("sidebar-header")
            .h(PANEL_HEADER_HEIGHT)
            .w_full()
            .items_center()
            .overflow_hidden()
            .rounded(cx.theme().radius)
            .text_xs()
            .text_color(cx.theme().sidebar_foreground)
            .children(active.map(view_title))
            .into_any_element()
    }
}

/// 标题:当前视图的名字。
fn view_title(view: SidebarView) -> AnyElement {
    div()
        .min_w_0()
        .h_full()
        .flex()
        .items_center()
        .child(ellipsis_label(view.label().into()))
        .into_any_element()
}
