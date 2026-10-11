//! 应用自有的可复用 UI 组件(gpui-kit 组件之外的补充)。
//!
//! 摆放约定与 gpui-kit 一致:每个组件一个文件,由使用处(如 [`crate::sidebar_panel`])装配。
//!
//! 这里的组件都基于 `gpui_kit::base` 搭、缺失处自绘,只从 `component` 取图标与悬停提示。

mod activitybar;
mod sidebar;

pub(crate) use activitybar::{ACTIVITY_BAR_WIDTH, ActivityBar, ActivityBarItem};
pub(crate) use sidebar::{Sidebar, SidebarRow};
