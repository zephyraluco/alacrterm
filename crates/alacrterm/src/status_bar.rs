//! 主窗口和预览窗口共用的状态栏布局。

use gpui::{Pixels, Styled as _, px};
use gpui_kit::component::status_bar::StatusBar;

pub(crate) const STATUS_BAR_ITEM_HEIGHT: Pixels = px(22.);
const STATUS_BAR_PADDING: Pixels = px(4.);

pub(crate) fn new_status_bar() -> StatusBar {
    StatusBar::new()
        .w_full()
        .flex_none()
        .py(STATUS_BAR_PADDING)
}
