//! 底部**整窗**的状态栏：**独占窗口最后一行**（横跨活动栏 + 全部列），目前是**空条**。
//! 侧边栏铺到它的上沿为止、没有自己的状态栏（那两枚「新建会话 / 新建文件夹」按钮
//! 已进会话视图的右键菜单，见 [`crate::sidebar_panel`]）。
//!
//! 这条栏曾经显示当前会话的指标（连接状态 / 连接目标 / 进程 CPU / 内存 / 网络速率），
//! 已按需求整体移除；连带删掉的还有 `status_metrics` 采样任务（否则会留下一个每 1.5s
//! 凭空重绘整棵树、却什么都不显示的定时器）。

use gpui::{AnyElement, IntoElement, Pixels, Styled as _, px};
use gpui_kit::component::status_bar::StatusBar;

/// 这条状态栏的高度（`StatusBar` 自身是按内容撑高的，这里钉死）。
pub(crate) const STATUS_BAR_HEIGHT: Pixels = px(28.);

/// 一条空状态栏（中间列那条）。
pub(crate) fn empty_status_bar() -> AnyElement {
    StatusBar::new()
        .h(STATUS_BAR_HEIGHT)
        .w_full()
        .into_any_element()
}
