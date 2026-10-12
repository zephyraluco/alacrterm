//! 底部的**整窗**状态栏（独占窗口最后一行）。

use gpui::{AnyElement, IntoElement, Pixels, Styled as _, px};
use gpui_kit::component::status_bar::StatusBar;

pub(crate) const STATUS_BAR_HEIGHT: Pixels = px(28.);

pub(crate) fn empty_status_bar() -> AnyElement {
    StatusBar::new()
        .h(STATUS_BAR_HEIGHT)
        .w_full()
        .into_any_element()
}
