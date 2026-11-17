//! Shared header presentation for business panels that use a title strip.
use super::{PANEL_HEADER_HEIGHT, shared::ellipsis_label};
use gpui::{
    AnyElement, App, InteractiveElement as _, IntoElement, ParentElement as _, Styled as _,
};
use gpui_kit::component::{ActiveTheme as _, h_flex};
pub(super) fn panel_header(title: &'static str, cx: &App) -> AnyElement {
    h_flex()
        .id("sidebar-header")
        .h(PANEL_HEADER_HEIGHT)
        .w_full()
        .items_center()
        .overflow_hidden()
        .rounded(cx.theme().radius)
        .text_xs()
        .text_color(cx.theme().sidebar_foreground)
        .child(ellipsis_label(title.into()))
        .into_any_element()
}
