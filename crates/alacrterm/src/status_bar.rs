//! 状态栏：每一列最下面的一条，只跟着「这一列有没有内容」走。
//!
//! 两处用它，gpui-kit 的同一个 [`StatusBar`] 组件、同一个高度（见 [`STATUS_BAR_HEIGHT`]）：
//!
//! - **中间列**：终端区下方，由 [`crate::AppRoot::render`] 摆进去，且**只在有标签页时**摆
//!   —— 欢迎页（没有终端）时整条不存在；
//! - **最左整列里的侧边栏**：面板下沿，由 `sidebar_panel::activity_bar::left_column` 经
//!   [`crate::components::Sidebar::status_bar`] 挂上，侧边栏折叠时不摆。
//!
//! 两条都是**空条**。文件管理器那些按钮不在状态栏里 —— 它们摆在自己面板**顶部的工具栏**上
//! （见 `sidebar_panel::activity_bar::files_toolbar`）。
//!
//! 两列各自以一条同高的状态栏收尾（各自独占自己那一列，不再横跨整窗）。

use gpui::{AnyElement, IntoElement, Pixels, Styled as _, px};
use gpui_kit::component::status_bar::StatusBar;

pub(crate) const STATUS_BAR_HEIGHT: Pixels = px(28.);

/// 一条空状态栏（底色 / 上边框 / 字号都由组件给）。
///
/// ⚠️ `.flex_none()`：定高的一条，别被同容器里那个 `flex_1` 兄弟压扁。
pub(crate) fn empty_status_bar() -> AnyElement {
    StatusBar::new()
        .h(STATUS_BAR_HEIGHT)
        .w_full()
        .flex_none()
        .into_any_element()
}
