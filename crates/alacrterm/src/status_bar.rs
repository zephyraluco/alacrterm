//! 底部**中间列**的公共状态栏（常驻，不随标签页清空而消失）：右端是当前会话指标
//! —— 连接状态 / 连接目标 / 会话进程 CPU / 内存 / 系统网络速率，数值由
//! [`crate::status_metrics`] 每 1.5s 采样一次；没有会话时显示「无会话」。
//!
//! 左右两侧边栏各有一条自己的状态栏（在 [`crate::sidebar_panel`] 里，目前是空条），
//! 三条统一用 [`STATUS_BAR_HEIGHT`] 保持等高；侧边栏折叠开关与「设置」入口都在标题栏。

use gpui::{AnyElement, App, Context, IntoElement, ParentElement as _, Pixels, Styled as _, div, px};
use gpui_kit::component::{ActiveTheme as _, h_flex, status_bar::StatusBar};

use crate::status_metrics::{SessionMetrics, format_bytes, format_rate};
use crate::terminal_panel::SessionTarget;
use crate::AppRoot;

/// 三条状态栏统一的高度（`StatusBar` 自身是按内容撑高的，三栏内容不同会不齐）。
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
/// - 连接状态是圆点 + 文字（运行中用 success 色、已结束用弱化色）；
/// - CPU / 内存取自**当前会话进程**（未就绪或已退出时显示 `--`）；
/// - 网络是**系统整体**速率（sysinfo 无法按进程统计流量）。
///
/// `exited` 来自会话结束事件，比等下一次采样更及时。
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
        // 还没采样到（刚启动 / PTY 未就绪）
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

    // 小圆点直接用文本「●」：与状态栏文字同高，省掉图标与额外布局。
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
    /// 中间列底部的公共状态栏：只放当前会话指标。
    pub(crate) fn render_status_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        // 没有会话时给一句占位文案。
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
