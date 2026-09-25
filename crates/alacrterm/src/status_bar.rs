//! 底部**中间列**的公共状态栏：目前是**空条** —— 只为与左右两侧边栏各自的状态栏保持等高
//! （那两条在 [`crate::sidebar_panel`] 里，其中「会话」视图会给它两端放新建按钮）。
//!
//! 这条栏曾经显示当前会话的指标（连接状态 / 连接目标 / 进程 CPU / 内存 / 网络速率），
//! 已按需求整体移除；连带删掉的还有 `status_metrics` 采样任务（否则会留下一个每 1.5s
//! 凭空重绘整棵树、却什么都不显示的定时器）。

use gpui::{AnyElement, IntoElement, Pixels, Styled as _, px};
use gpui_kit::component::status_bar::StatusBar;

/// 三条状态栏统一的高度（`StatusBar` 自身是按内容撑高的，三栏内容不同会不齐）。
pub(crate) const STATUS_BAR_HEIGHT: Pixels = px(28.);

/// 一条空状态栏（中间列那条）。
pub(crate) fn empty_status_bar() -> AnyElement {
    StatusBar::new()
        .h(STATUS_BAR_HEIGHT)
        .w_full()
        .into_any_element()
}
