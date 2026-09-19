//! 底部**中间列**的公共状态栏（常驻，不随标签页清空而消失）。
//!
//! 窗口底部不再是整窗一条：三列各占自己的底边——
//! - **左栏 / 右栏**：各自的状态栏在 [`crate::sidebar_panel`] 里，宽度随侧边栏一起变化
//!   （侧边栏拖宽 / 拖窄时实时跟随，因为它们在同一个列容器里）；
//!   **两条当前都是空条**（左侧原来的视图图标已迁到侧边栏顶部的段控、
//!   右侧原来的「会话信息」标识已移除），保留只为与中间这条等高对齐。
//! - **中间列**（本模块）：终端区下方这条公共状态栏。
//!
//! 三条状态栏的**高度必须一致**（三栏底边对齐，谁矮一点顶部就会错位）：
//! 统一用 [`STATUS_BAR_HEIGHT`]，因为 `StatusBar` 的高度是内容高度决定的——
//! 含文字的那条（指标 / 侧边栏名称）自然比只有 16px 图标按钮的那条高。
//!
//! 本模块只负责中间这条：
//! - 右端：当前会话指标 —— 连接状态 / 连接目标 / 会话进程 CPU / 内存 / 系统网络速率，
//!   数值由 [`crate::status_metrics`] 每 1.5s 采样一次；没有会话时只显示一句「无会话」。
//!
//! 两侧边栏的折叠 / 展开开关与「设置」入口都在标题栏
//! （[`crate::AppRoot::render_sidebar_toggles`] / [`crate::AppRoot::render`]），
//! 标题栏常驻，所以状态栏里不再需要任何恢复入口。
//!
//! 指标各段的排版（弱化色标签 + 常规色数值、`·` 分隔）沿用原先终端状态栏的实现。

use gpui::{AnyElement, App, Context, IntoElement, ParentElement as _, Pixels, Styled as _, div, px};
use gpui_kit::component::{ActiveTheme as _, h_flex, status_bar::StatusBar};

use crate::AppRoot;
use crate::status_metrics::{SessionMetrics, format_bytes, format_rate};
use crate::SessionTarget;

/// 三条状态栏统一的高度。
///
/// `StatusBar` 的高度由内容撑出（`py_1` + 内容行高），而三栏的内容并不一样：
/// 左右两条里只有 16px 高的图标按钮，中间那条含一行文字（`text_xs` 的行高约 20px）。
/// 三栏底边是对齐的，谁矮一点它的顶边就会低一截、看上去“短了一截”。
/// 这里统一定死为含文字那条的自然高度，三条就完全等高（图标按钮在中间垂直居中）。
pub(crate) const STATUS_BAR_HEIGHT: Pixels = px(28.);

/// 状态栏里的一项指标：弱化色的标签 + 常规色的数值。
fn metric_item(label: &'static str, value: String, cx: &App) -> AnyElement {
    h_flex()
        .gap_1()
        .flex_shrink_0()
        .child(div().text_color(cx.theme().muted_foreground).child(label))
        .child(value)
        .into_any_element()
}

/// 状态栏项目之间的分隔点。
fn metric_separator(cx: &App) -> AnyElement {
    div()
        .flex_shrink_0()
        .text_color(cx.theme().muted_foreground)
        .child("·")
        .into_any_element()
}

/// 状态栏右段：连接状态 · 连接目标 · CPU · 内存 · 网络。
///
/// - 连接状态由小圆点 + 文字表示：进程仍在运行用 success 色，已结束用弱化色；
/// - CPU / 内存取自**当前会话进程**（进程未就绪或已退出时显示 `--`）；
/// - 网络是**系统整体**速率（sysinfo 无法按进程统计流量，详见 `status_metrics` 模块）。
///
/// `exited` 来自 [`TerminalView::has_exited`](terminal_view::TerminalView::has_exited)：
/// 会话进程结束的事件一到就能立刻显示「已断开」，不必等下一次指标采样
/// （最多 1.5s）才发现进程没了。
fn render_session_metrics(
    target: &SessionTarget,
    metrics: SessionMetrics,
    exited: bool,
    cx: &App,
) -> AnyElement {
    let (state_text, state_color) = match (exited, metrics.alive) {
        // 会话结束事件已到达（最及时），或采样发现进程已消失。
        (true, _) | (false, Some(false)) => ("已断开", cx.theme().muted_foreground),
        (false, Some(true)) => ("运行中", cx.theme().success),
        // 还没采样到（刚启动 / PTY 未就绪）：显示中性状态，避免误报断开。
        (false, None) => ("启动中", cx.theme().muted_foreground),
    };

    let cpu = metrics
        .cpu_percent
        .map(|cpu| format!("{cpu:.1}%"))
        .unwrap_or_else(|| "--".to_string());
    let memory = metrics
        .memory_bytes
        .map(format_bytes)
        .unwrap_or_else(|| "--".to_string());

    // 小圆点用文本「●」绘制：与状态栏文字同高，省掉图标与额外布局。
    let state_dot = div()
        .flex_shrink_0()
        .text_color(state_color)
        .child("●")
        .into_any_element();

    h_flex()
        .gap_2()
        .items_center()
        .child(
            h_flex()
                .gap_1()
                .flex_shrink_0()
                .child(state_dot)
                .child(state_text),
        )
        .child(metric_separator(cx))
        .child(metric_item("连接", target.label(), cx))
        .child(metric_separator(cx))
        .child(metric_item("CPU", cpu, cx))
        .child(metric_separator(cx))
        .child(metric_item("内存", memory, cx))
        .child(metric_separator(cx))
        .child(metric_item(
            "网络",
            format!(
                "↓{} ↑{}",
                format_rate(metrics.net_rx_per_sec),
                format_rate(metrics.net_tx_per_sec)
            ),
            cx,
        ))
        .into_any_element()
}

impl AppRoot {
    /// 中间列底部的**公共状态栏**（常驻中间列底部，见模块文档）。
    ///
    /// 只放当前会话指标：两侧边栏的折叠开关与「设置」都在标题栏，标题栏常驻
    /// ⇒ 这里不必再为折叠掉的侧边栏补「展开」入口。
    pub(crate) fn render_status_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        // 当前会话指标：没有会话（标签页全部关闭）时给一句占位文案。
        let metrics = match self.terminals.get(self.active) {
            Some(session) => render_session_metrics(
                session.target(cx),
                self.monitor.metrics(),
                session.view.read(cx).has_exited(),
                cx,
            ),
            None => div()
                .flex_shrink_0()
                .child("无会话")
                .into_any_element(),
        };

        StatusBar::new()
            .right(metrics)
            .h(STATUS_BAR_HEIGHT)
            .w_full()
            .into_any_element()
    }
}
