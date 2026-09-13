//! 右侧容器：多会话标签栏 + 终端视图。
//!
//! - **标签栏**：**自绘**（见 [`crate::tab_bar`]）——图标 + 标题 + 关闭按钮全部用
//!   gpui 原语绘制，未使用 gpui-kit 的 `TabBar` / `Tab` 组件；标签多了横向滚动，
//!   右端固定一枚「+」新建终端。
//! - **终端**：`TerminalView` 实体直接挂在终端区里，尺寸随容器分配（不画卡片边框，
//!   以免在标签栏底边线下方多出一条平行横线）。
//!
//! 指标状态栏（连接状态 / CPU / 内存 / 网络）已整体搬到 [`crate::status_bar`]：
//! 那是**中间列的公共状态栏**（不随本容器的消失而消失）。
//! 会话进程结束时仍显示「已断开」：终端网格与标签都保留（ssh 报的断开原因
//! 不会被丢掉），由用户自行关闭或新建。
//!
//! 标签页可全部关闭；**最后一个会话关闭后整个容器一起关闭**（标签栏 / 终端
//! 全部消失，由 [`AppRoot::render`] 决定不再渲染本容器，中间列改显示欢迎页
//! ——见 [`crate::AppRoot::render_welcome`]），
//! 之后可在欢迎页里、或从侧边栏会话条目的右键菜单「新建终端」重新打开。
//!
//! 会话的生命周期（新建 / 激活 / 关闭）也集中在本模块，作为终端的「单一入口」；
//! 侧边栏的会话列表经由 [`AppRoot::set_active_tab`] 复用同一套逻辑。

use gpui::{
    AnyElement, AppContext as _, Context, Focusable as _, IntoElement, ParentElement as _,
    Styled as _, Window, div,
};
use gpui_kit::component::v_flex;
use terminal_view::TerminalView;
use util::shell::Shell;

use crate::{AppRoot, Session, SessionRequest, SessionTarget};

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
        // 焦点策略集中在应用层（点击终端之外时焦点会离开终端，见
        // `AppRoot::on_background_mouse_down`），所以新会话要显式聚焦，否则键盘没有去处。
        let focus = view.focus_handle(cx);
        window.focus(&focus, cx);
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
            // 全部标签已关闭：无会话可激活，中间列显示欢迎页（见 `render_welcome`）。
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
    /// 允许关闭全部标签：最后一个会话关闭后终端容器整体消失、中间列显示欢迎页（见
    /// [`Self::render_welcome`]），用户可在其中新建会话。
    /// 实体移除后 `Terminal` 的 Drop 会关闭 PTY 并终止子进程。
    pub(crate) fn close_terminal(&mut self, index: usize, cx: &mut Context<Self>) {
        // 点击 × 后事件仍可能冒泡到标签的 on_click（自绘标签栏虽然在关闭按钮里
        // `stop_propagation` 了，这里仍做越界校验作为防御）——下标可能已失效。
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

        // 标签栏是**自绘**的（`tab_bar` 模块）：图标 + 标题 + 关闭按钮都用 gpui 原语，
        // 不再使用 gpui-kit 的 `TabBar` / `Tab`（旧实现里标签栏自带 `menu(true)` 菜单键，
        // 以及为绕过 `lock_scroll_axis` 而手写的垂直滚轮→横向映射，都已随之删除）。

        // 终端区：直接挂载当前会话的 `TerminalView`。
        //
        // 刻意**不画边框 / 圆角**：标签栏已经贴边并自带一条底边线，
        // 这里再画一圈边框就会在它下方 8px（pane 的内边距）处多出一条平行横线，
        // 看上去像是重复的分隔线。不画边框后，终端背景与 pane 背景同色，
        // 选中标签的底色与下方区域自然连成一体。
        //
        // ⚠️ 内边距那层必须是 **flex 列容器**（`v_flex`），不能是普通 `div()`：
        // `TerminalView` 是 `size_full`，百分比高度要解析到**确定的高度**上；
        // 若外层是块级 div，`terminal_area` 的 `flex_1()` 不生效、高度退化为 auto，
        // 终端就会被压成 0 高（表现为「终端一片空白，什么都不显示」）。
        let terminal_area = v_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .child(active.view.clone());

        // 终端 pane：上方自绘标签栏（贴边，自带底边线）+ 下方终端区（留白）。
        // overflow_hidden：pane 无 overflow 时，taffy 的自动最小尺寸 = 内容宽
        // （含所有标签的总宽），标签一多 pane 会被撑出可视区；设为 hidden 后
        // 最小尺寸归零，宽度完全由行分配——标签栏内部再横向滚动。
        // flex_1 + min_h_0：与下方状态栏同处一列，需能收缩。
        let terminal_pane = v_flex()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .child(self.render_terminal_tab_bar(cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .p_2()
                    .overflow_hidden()
                    .child(terminal_area),
            );

        v_flex()
            .h_full()
            .w_full()
            .overflow_hidden()
            .child(terminal_pane)
            .into_any_element()
    }
}
