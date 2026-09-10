//! 右侧容器：多会话标签栏 + 终端视图 + 终端状态栏。
//!
//! - **标签栏**：动态标签（图标前缀 + 标题 + × 关闭后缀），菜单键固定在右端兜底；
//!   标签溢出时内部横向裁剪 / 滚动，并额外接管垂直滚轮（`TabBar` 内部
//!   `lock_scroll_axis` 禁用了「垂直滚轮 → 横向」的自动映射）。
//! - **终端**：`TerminalView` 实体直接挂在卡片内，尺寸随容器分配。
//! - **状态栏**：只展示指标——连接状态 / 连接目标 / CPU / 内存 / 网络
//!   （不显示终端名称：名称已出现在标签页与侧边栏，状态栏再显示是重复）。
//!   会话进程结束时显示「已断开」：终端网格与标签都保留（ssh 报的断开原因
//!   不会被丢掉），由用户自行关闭或新建。
//!
//! 标签页可全部关闭；**最后一个会话关闭后整个容器一起关闭**（标签栏 / 终端 /
//! 状态栏全部消失，由 [`AppRoot::render`] 决定不再渲染本容器），
//! 用户可随时从侧边栏底部的「新建终端」重新打开。
//!
//! 会话的生命周期（新建 / 激活 / 关闭）也集中在本模块，作为终端的「单一入口」；
//! 侧边栏的会话列表经由 [`AppRoot::set_active_tab`] 复用同一套逻辑。

use gpui::{
    AnyElement, App, AppContext as _, Context, InteractiveElement as _, IntoElement,
    ParentElement as _, ScrollWheelEvent, Styled as _, Window, div, point, px,
};
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, h_flex,
    button::{Button, ButtonVariants as _},
    status_bar::StatusBar,
    tab::{Tab, TabBar},
    v_flex,
};
use terminal_view::TerminalView;
use util::shell::Shell;

use crate::assets::IconName;
use crate::status_metrics::{SessionMetrics, format_bytes, format_rate};
use crate::{AppRoot, Session, SessionRequest, SessionTarget};

/// 状态栏里的一项指标：弱化色的标签 + 常规色的数值。
fn metric_item(label: &'static str, value: String, cx: &App) -> AnyElement {
    h_flex()
        .gap_1()
        .flex_shrink_0()
        .child(
            div()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
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

/// 状态栏右段：连接状态 · CPU · 内存 · 网络。
///
/// - 连接状态由小圆点 + 文字表示：进程仍在运行用 success 色，已结束用弱化色；
/// - CPU / 内存取自**当前会话进程**（进程未就绪或已退出时显示 `--`）；
/// - 网络是**系统整体**速率（sysinfo 无法按进程统计流量，详见 `status_metrics` 模块）。
///
/// `exited` 来自 [`TerminalView::has_exited`]：会话进程结束的事件一到就能立刻显示
/// 「已断开」，不必等下一次指标采样（最多 1.5s）才发现进程没了。
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
    /// 新建一个本地终端会话（默认系统 shell）。
    pub(crate) fn spawn_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.spawn_session(
            SessionRequest {
                name: None,
                shell: Shell::System,
                target: SessionTarget::Local,
            },
            window,
            cx,
        );
    }

    /// 按给定参数新建会话（PTY 在后台启动），并订阅其事件用于刷新界面。
    ///
    /// 参数来自 [`ConnectionForm`](crate::connection_dialog::ConnectionForm)：
    /// 显示名、要启动的 shell（本地 shell 或 `ssh`）、连接目标（状态栏展示用）。
    pub(crate) fn spawn_session(
        &mut self,
        request: SessionRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SessionRequest {
            name,
            shell,
            target,
        } = request;
        let view = cx.new(|cx| TerminalView::new(None, shell, window, cx));
        cx.observe(&view, |_, _, cx| cx.notify()).detach();
        self.terminals.push(Session {
            view,
            name,
            target,
        });
        self.set_active_tab(self.terminals.len() - 1, cx);
    }

    /// 激活指定的终端会话标签，并把标签栏滑动到该标签。
    ///
    /// `ScrollHandle::scroll_to_item` 在下一帧 prepaint 时生效：仅做最小滚动，
    /// 让选中的标签进入可视范围（标签已在视野内时不动）。
    /// 所有激活路径（标签点击 / 侧边栏会话项 / 新建会话）都应经由本方法。
    pub(crate) fn set_active_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.terminals.is_empty() {
            // 全部标签已关闭：无会话可激活，界面处于空白背景板状态。
            self.active = 0;
            cx.notify();
            return;
        }
        self.active = index.min(self.terminals.len() - 1);
        self.tab_scroll_handle.scroll_to_item(self.active);
        cx.notify();
    }

    /// 关闭一个终端会话（标签页 × 按钮，参考官方 Dynamic Tabs / Closeable Tabs 示例）。
    ///
    /// 允许关闭全部标签：最后一个会话关闭后界面进入空白背景板状态（见
    /// [`Self::render_terminal_container`]），用户可在其中新建会话。
    /// 实体移除后 `Terminal` 的 Drop 会关闭 PTY 并终止子进程。
    pub(crate) fn close_terminal(&mut self, index: usize, cx: &mut Context<Self>) {
        // 点击 × 后事件会冒泡到 TabBar 的 on_click 且下标可能已失效，需自行校验。
        if index >= self.terminals.len() {
            return;
        }
        self.terminals.remove(index);
        // 选中下标调整逻辑与官方 close_tab 示例一致（末位用 saturating 防止 usize 下溢）。
        if self.active >= index && self.active > 0 {
            self.active -= 1;
        }
        if self.active >= self.terminals.len() {
            self.active = self.terminals.len().saturating_sub(1);
        }
        if !self.terminals.is_empty() {
            self.tab_scroll_handle.scroll_to_item(self.active);
        }
        cx.notify();
    }

    /// 终端容器：标签栏 + 终端卡片 + 底部状态栏。
    pub(crate) fn render_terminal_container(&self, cx: &mut Context<Self>) -> AnyElement {
        // 前置条件：至少存在一个会话。全部标签页关闭后整个终端容器不再渲染
        // （见 `AppRoot::render`），本方法不会被调用；此处只做防御性检查，
        // 避免 `self.active` 越界时 panic。
        let Some(active) = self.terminals.get(self.active) else {
            return div().into_any_element();
        };

        // 终端会话标签页（参考官方 Tabs 示例的「Dynamic Tabs」）：
        // 每个标签由图标前缀 + 标题 + × 关闭后缀组成。
        // w_full：显式占满父宽——否则标签栏按内容收缩，标签一多会撑出可视区
        // 并把右侧菜单键推走；约束住后内部自动横向裁剪 / 滚动，菜单键固定在右端。
        let tab_bar = TabBar::new("terminal-tabs")
            .w_full()
            .menu(true)
            // 关联滚动句柄：激活会话时 scroll_to_item 精确滑动到选中标签。
            .track_scroll(&self.tab_scroll_handle)
            .selected_index(self.active)
            .on_click(cx.listener(|this, index: &usize, _, cx| {
                // 点击标签内 × 关闭会话后，事件仍会冒泡到此处且下标可能已失效，需钳制。
                this.set_active_tab(*index, cx);
            }))
            .children(
                self.terminals.iter().enumerate().map(|(ix, session)| {
                    Tab::new()
                        .px_2()
                        .prefix(Icon::new(IconName::SquareTerminal))
                        // 显示名：用户命名优先，否则终端自身标题。
                        .label(session.title(cx))
                        .suffix(
                            Button::new(format!("close-tab-{ix}"))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .tooltip("关闭会话")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.close_terminal(ix, cx);
                                })),
                        )
                }),
            );

        // 滚轮滚动标签栏：TabBar 内部 lock_scroll_axis 禁用了「垂直滚轮 → 横向」的
        // 自动映射，因此在外层把滚轮增量手动写入 ScrollHandle（横向偏移）；
        // 触摸板横向滚动仍由标签栏内部处理，不会重复。
        let tab_scroll_handle = self.tab_scroll_handle.clone();
        let tab_bar_area = div()
            .id("tab-bar-area")
            .w_full()
            .on_scroll_wheel(move |event: &ScrollWheelEvent, window, _cx| {
                let dy = event.delta.pixel_delta(window.line_height()).y;
                if dy == px(0.) {
                    return;
                }
                let max = tab_scroll_handle.max_offset().x;
                // GPUI 的滚轮增量已包含方向，直接映射到横向偏移。
                let next = (tab_scroll_handle.offset().x + dy).clamp(-max, px(0.));
                tab_scroll_handle.set_offset(point(next, px(0.)));
                window.refresh();
            })
            .child(tab_bar);

        // 终端卡片：挂载当前会话的 `TerminalView`。
        let terminal_card = div()
            .flex_1()
            .border_1()
            .border_color(cx.theme().border)
            .rounded_md()
            .overflow_hidden()
            .child(active.view.clone());

        // 终端 pane：上方标签栏 + 下方终端卡片。
        // overflow_hidden：pane 无 overflow 时，taffy 的自动最小尺寸 = 内容宽
        // （含所有标签的总宽），标签一多 pane 会被撑出可视区并把 TabBar(w_full)
        // 与菜单键一起推走；设为 hidden 后最小尺寸归零，宽度完全由行分配。
        // flex_1 + min_h_0：与下方状态栏同处一列，需能收缩。
        let terminal_pane = v_flex()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .p_2()
            .gap_2()
            .child(tab_bar_area)
            .child(terminal_card);

        // 状态栏：只放指标——连接状态 + 连接目标 + 会话进程的 CPU / 内存 + 系统网络速率，
        // 全部靠右显示（不显示终端名称，名称已在标签页 / 侧边栏里）。
        // 指标由后台采样任务每 1.5s 更新一次（见 `status_metrics` 模块）。
        let status_bar = StatusBar::new()
            .right(render_session_metrics(
                &active.target,
                self.monitor.metrics(),
                active.view.read(cx).has_exited(),
                cx,
            ))
            .w_full();

        v_flex()
            .h_full()
            .w_full()
            .overflow_hidden()
            .child(terminal_pane)
            .child(status_bar)
            .into_any_element()
    }
}
