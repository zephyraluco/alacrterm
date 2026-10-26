//! 终端标签页：生命周期（新建 / 激活 / 关闭 / 分屏）与它们在 dock 里的面板 [`TabPane`]；
//! 标签表 [`AppRoot::tabs`] 与 dock 面板一一对应、顺序一致。入口 [`AppRoot::spawn_terminal`] /
//! [`AppRoot::spawn_session`] / [`AppRoot::split_active_session`]。
//!
//! 一个标签页 = 标签栏上的一枚标签，内部是一棵**分屏树**（见 [`crate::pane_layout`]）：
//! 每格一台终端（各自一个 PTY），整页共用同一条标签栏（kitty / ghostty 的模型）。
//!
//! 分屏**沿用本页的会话**：新格子照本页的 [`TerminalTarget`] 再建一台（远端仍是远端，
//! 本地仍是同一个 shell），并尽量落在当前格子的工作目录里 —— 见
//! [`AppRoot::split_active_session`]。

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext as _, Axis, Context, Div, DragMoveEvent, Empty, Entity, EntityId,
    EventEmitter, FocusHandle, Focusable, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, WeakEntity, Window, div,
    relative,
};
use gpui_kit::component::{
    button::Button,
    dock::{
        BasePanel, DockPlacement, InsertTarget, Panel, PanelControl, PanelEvent, PanelId, TabGroup,
        panel_handle,
    },
    h_flex,
    resizable::resize_handle,
    v_flex,
};
use terminal::{SshFs, SessionId, TerminalTarget};
use terminal_view::{RenderSettings, TerminalView};

use crate::actions::NewLocalTerminal;
use crate::assets::IconName;
use crate::config;
use crate::dialog::host_key;
use crate::pane_layout::{Dir, Layout, ratio_at};
use crate::AppRoot;

pub(crate) use session::SessionRequest;

impl AppRoot {
    /// 新建本地终端标签页。
    pub(crate) fn spawn_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.spawn_session(
            SessionRequest {
                name: None,
                target: TerminalTarget::system_shell(),
            },
            window,
            cx,
        );
    }

    /// 按参数新建标签页并挂成 dock 面板。
    pub(crate) fn spawn_session(
        &mut self,
        request: SessionRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 新标签页进哪一组（dock 被拖成两个标签组时才用得上）。
        let group = self
            .pending_session_group
            .take()
            .or_else(|| self.session_group(self.active, cx));
        self.spawn_session_in(request, group, window, cx);
    }

    fn session_group(&self, index: usize, cx: &App) -> Option<WeakEntity<TabGroup>> {
        self.tabs.get(index).and_then(|tab| tab.read(cx).group())
    }

    /// [`Self::spawn_session`] 的实现（`group` = 目标标签组）。
    fn spawn_session_in(
        &mut self,
        request: SessionRequest,
        group: Option<WeakEntity<TabGroup>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SessionRequest { name, target } = request;
        let view = self.new_terminal(None, target.clone(), window, cx);
        let root = cx.weak_entity();
        let pane = cx.new(|cx| TabPane::new(view, name.map(Into::into), target, root, window, cx));
        let handle = panel_handle(pane.clone());
        self.dock.update(cx, |area, cx| {
            area.add_panel_view(handle, DockPlacement::Center, None, window, cx);
        });

        // `add_panel_view` 只会塞进第一个标签组。
        if let Some(node) = group.and_then(|group| group.upgrade().map(|group| group.read(cx).node()))
        {
            let panel = panel_handle(pane.clone()).panel_id(cx);
            self.dock.update(cx, |area, cx| {
                area.move_panel(
                    panel,
                    InsertTarget::Tabs {
                        node,
                        ix: None,
                        activate: true,
                    },
                    window,
                    cx,
                );
            });
        }

        self.tabs.push(pane);
        // 按 dock 的顺序重排。
        self.active = self.tabs.len() - 1;
        self.sync_tabs_with_dock(cx);
        self.sync_active_context(window, cx);
        cx.notify();
    }

    /// 在「当前」标签页里再开一格（`Ctrl+Shift+D` / `Ctrl+Shift+S`）。
    ///
    /// 新格子**沿用本页的会话**（[`TabPane::target`]）而不是另起本地终端；本地会话还会
    /// 落在当前格子的工作目录里（远端拿不到远端路径，见 [`TerminalView::working_directory`]）。
    pub(crate) fn split_active_session(
        &mut self,
        axis: Axis,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.active_tab_index(window, cx) else {
            return;
        };
        let Some(tab) = self.tabs.get(index).cloned() else {
            return;
        };
        let (target, working_directory) = tab.read(cx).split_params(cx);
        let view = self.new_terminal(working_directory, target, window, cx);
        tab.update(cx, |tab, cx| tab.add_pane(view, axis, window, cx));
        cx.notify();
    }

    /// 建一台终端视图（顺手订好重绘）。
    ///
    /// `working_directory` = 起始工作目录（本地会话用；远端忽略）。
    fn new_terminal(
        &self,
        working_directory: Option<PathBuf>,
        target: TerminalTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TerminalView> {
        let settings = Arc::new(config::settings(cx).render.clone());
        let root = cx.weak_entity();
        // 主机密钥确认弹窗（仅远端）。
        let handler = target
            .is_ssh()
            .then(|| host_key::host_key_prompt_handler(root));
        let options = crate::shell_integration::launch_options(settings.suggestions.remote_integration);
        let view = cx.new(|cx| {
            TerminalView::new(working_directory, target, handler, options, settings, window, cx)
        });
        // 标题 / 焦点 / 连接状态变化时重绘（标签栏的文字也跟着它走）。
        cx.observe(&view, |_, _, cx| cx.notify()).detach();
        view
    }

    /// 把渲染参数推给所有存活标签页的每一格。
    pub(crate) fn apply_render_settings(&mut self, settings: RenderSettings, cx: &mut App) {
        let settings = Arc::new(settings);
        for tab in &self.tabs {
            tab.update(cx, |tab, cx| tab.set_render_settings(settings.clone(), cx));
        }
    }

    /// 按面板 id 同步当前标签页下标。
    pub(crate) fn set_active_pane(&mut self, pane: EntityId, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.entity_id() == pane) else {
            return;
        };
        if self.active == index {
            return;
        }
        self.active = index;
        cx.notify();
    }

    /// 关闭一个标签页（连带它内部的所有格子）。
    pub(crate) fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(index).cloned() else {
            return;
        };
        // 先让 dock 摘掉面板。
        self.dock
            .update(cx, |area, cx| area.remove_panel(tab, window, cx));
        self.tabs.remove(index);
        if self.active >= index && self.active > 0 {
            self.active -= 1;
        }
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        }
        // 顺序跟着 dock 走。
        self.sync_tabs_with_dock(cx);
        self.sync_active_context(window, cx);
        cx.notify();
    }

    /// 按面板 id 关闭标签页。
    pub(crate) fn close_panel_id(
        &mut self,
        panel: PanelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self
            .tabs
            .iter()
            .position(|tab| panel_handle(tab.clone()).panel_id(cx) == panel)
        else {
            return;
        };
        self.close_tab(index, window, cx);
    }

    /// 把标签表同步成 dock 的样子（顺序 / 成员）。
    pub(crate) fn sync_tabs_with_dock(&mut self, cx: &mut Context<Self>) {
        let Some(tree) = self.dock.read(cx).layout(DockPlacement::Center) else {
            return;
        };
        let order: Vec<PanelId> = tree.panels().collect();
        let active_pane = self.tabs.get(self.active).map(|tab| tab.entity_id());
        let mut ordered = Vec::with_capacity(self.tabs.len());
        for id in order {
            let Some(ix) = self
                .tabs
                .iter()
                .position(|tab| panel_handle(tab.clone()).panel_id(cx) == id)
            else {
                continue;
            };
            ordered.push(self.tabs.remove(ix));
        }
        let dropped = !self.tabs.is_empty();
        let before: Vec<EntityId> = self.tabs.iter().map(|tab| tab.entity_id()).collect();
        self.tabs = ordered;
        let after: Vec<EntityId> = self.tabs.iter().map(|tab| tab.entity_id()).collect();
        if !dropped && before == after {
            return;
        }
        // 当前标签页按面板找回下标。
        self.active = active_pane
            .and_then(|pane| self.tabs.iter().position(|tab| tab.entity_id() == pane))
            .unwrap_or_else(|| self.active.min(self.tabs.len().saturating_sub(1)));
        cx.notify();
    }

    /// 「当前」标签页下标：焦点所在的终端优先，否则用 `active`。
    ///
    /// 键位动作都是从键盘派发来的（焦点必然在某个终端里），但 `Window` 只有拿到窗口的
    /// 回调才有（见 [`AppRoot::defer_after_update`]），所以键位动作一律走这条路径而不用
    /// [`Self::active`] 直觉判断。
    pub(crate) fn active_tab_index(&self, window: &Window, cx: &App) -> Option<usize> {
        self.tabs
            .iter()
            .position(|tab| tab.read(cx).is_focused(window, cx))
            .or_else(|| (!self.tabs.is_empty()).then(|| self.active.min(self.tabs.len() - 1)))
    }

    /// 对「当前」标签页里聚焦的那一格做一件事（找不到就什么也不做）。
    ///
    /// 键位动作的另一条入口：不需要 `Window`，用 `active` 即可（渲染时按焦点更新过）。
    pub(crate) fn with_active_terminal(
        &self,
        cx: &mut App,
        action: impl FnOnce(&mut TerminalView, &mut Context<TerminalView>),
    ) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        tab.read(cx).focused_view().update(cx, action);
    }

    /// 关闭当前格子（`Ctrl+Shift+W`）；只剩一格时关掉整个标签页。
    pub(crate) fn close_active_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.active_tab_index(window, cx) else {
            return;
        };
        let Some(tab) = self.tabs.get(index).cloned() else {
            return;
        };
        let focused = tab.read(cx).focused_index();
        let remaining = tab.update(cx, |tab, cx| tab.close_pane(focused, window, cx));
        if remaining {
            cx.notify();
        } else {
            self.close_tab(index, window, cx);
        }
    }

    /// 把焦点挪到当前标签页里某个方向上的邻格（`Ctrl+Shift+方向键`）。
    pub(crate) fn focus_pane(&mut self, dir: Dir, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.active_tab_index(window, cx) else {
            return;
        };
        let Some(tab) = self.tabs.get(index).cloned() else {
            return;
        };
        tab.update(cx, |tab, cx| tab.focus_neighbor(dir, window, cx));
    }

    /// 前后切换标签页（`Ctrl+Tab` / `Ctrl+Shift+Tab`），到头折返。
    pub(crate) fn step_session(&mut self, delta: i32, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.tabs.len();
        if count == 0 {
            return;
        }
        let Some(current) = self.active_tab_index(window, cx) else {
            return;
        };
        let next = (current as i32 + delta).rem_euclid(count as i32) as usize;
        self.activate_session(next, window, cx);
    }

    /// 跳到第 `index` 个标签页：切到它所在的标签栏位置并聚焦它当前那一格。
    pub(crate) fn activate_session(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(index).cloned() else {
            return;
        };
        let focus = tab.read(cx).focus_handle(cx);
        let group = tab.read(cx).group().and_then(|group| group.upgrade());
        self.active = index;

        let Some(group) = group else {
            // 还没进过标签组（首个标签页）：直接聚焦它。
            window.focus(&focus, cx);
            self.sync_active_context(window, cx);
            cx.notify();
            return;
        };
        let panel = panel_handle(tab.clone()).panel_id(cx);
        let slot = group
            .read(cx)
            .panels()
            .iter()
            .position(|candidate| candidate.panel_id(cx) == panel);
        match slot {
            // 切标签页会顺带聚焦（`TabGroup::select_tab` → `focus_active_panel`）。
            Some(slot) => group.update(cx, |group, cx| group.select_tab(slot, window, cx)),
            None => window.focus(&focus, cx),
        }
        self.sync_active_context(window, cx);
        cx.notify();
    }
}

/// 一个标签页：一棵分屏树 + 它在 dock 里的面板。
pub(crate) struct TabPane {
    root: WeakEntity<AppRoot>,
    /// 用户填的名字；`None` = 用当前聚焦格子的终端标题。
    name: Option<SharedString>,
    /// 本页所有格子的连接目标（分屏照它再建一台，见 [`Self::split_params`]）。
    target: TerminalTarget,
    /// 本面板所在的标签组。
    group: Option<WeakEntity<TabGroup>>,
    /// 每格一台终端。
    panes: Vec<Entity<TerminalView>>,
    /// 与 `panes` 一一对应的订阅（终端标题 / 焦点变化时要重绘标签栏）。
    subs: Vec<Subscription>,
    /// 格子布局。
    layout: Layout,
    /// 下一个分叉 id（[`Layout::Split::node`] 用，单调递增就够）。
    next_node: u64,
    /// 当前聚焦的格子（下标进 `panes`）。
    focused: usize,
    /// [`Self::focus_handle`] 的兜底（格子列表为空时才用得上）。
    fallback_focus: FocusHandle,
}

impl TabPane {
    pub(crate) fn new(
        view: Entity<TerminalView>,
        name: Option<SharedString>,
        target: TerminalTarget,
        root: WeakEntity<AppRoot>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let sub = Self::observe_pane(&view, window, cx);
        Self {
            root,
            name,
            target,
            group: None,
            panes: vec![view],
            subs: vec![sub],
            layout: Layout::single(),
            next_node: 0,
            focused: 0,
            fallback_focus: cx.focus_handle(),
        }
    }

    fn observe_pane(
        view: &Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Subscription {
        cx.observe_in(view, window, |this, _view, window, cx| {
            this.sync_focus(window, cx);
            cx.notify();
            // 标题 / 焦点 / 连接状态 / 远端目录都可能刚变过：让根视图在下一帧重算上下文。
            // ⚠️ 不能在这里同步 `root.update(.. sync_active_context)` —— 本实体正在被更新，
            // 那会再读到自己（gpui 的重复租借检查）。
            if let Some(root) = this.root.upgrade() {
                let _ = root.update(cx, |root, cx| root.schedule_context_sync(cx));
            }
        })
    }

    pub(crate) fn group(&self) -> Option<WeakEntity<TabGroup>> {
        self.group.clone()
    }

    /// 当前聚焦格子的下标。
    pub(crate) fn focused_index(&self) -> usize {
        self.focused
    }

    /// 当前聚焦格子的终端视图。
    pub(crate) fn focused_view(&self) -> Entity<TerminalView> {
        self.panes
            .get(self.focused)
            .or_else(|| self.panes.first())
            .expect("标签页至少有一格")
            .clone()
    }

    /// 标签页显示名：用户填的名字，否则当前聚焦格子的终端标题。
    pub(crate) fn name(&self, cx: &App) -> SharedString {
        self.name
            .clone()
            .unwrap_or_else(|| self.focused_view().read(cx).title())
    }

    pub(crate) fn is_remote(&self) -> bool {
        self.target.is_ssh()
    }

    /// 分屏用的参数：本页的会话目标 + 当前格子的工作目录（远端没有本地路径 ⇒ `None`）。
    ///
    /// 这是「标签页内分屏应该在同一个环境里」的落点：远端标签页分屏还是连那台主机，
    /// 本地标签页分屏还是同一个 shell，并且尽量从当前目录开始。
    pub(crate) fn split_params(&self, cx: &App) -> (TerminalTarget, Option<PathBuf>) {
        let view = self.focused_view();
        let working_directory = view.read(cx).working_directory(cx);
        (self.target.clone(), working_directory)
    }

    /// 本页有格子持有焦点吗（根视图靠它认「当前标签页」）。
    pub(crate) fn is_focused(&self, window: &Window, cx: &App) -> bool {
        self.panes
            .iter()
            .any(|view| view.read(cx).is_focused(window))
    }

    /// 当前格子的 SFTP 句柄（远端会话才有）。
    pub(crate) fn remote_fs(&self, cx: &App) -> Option<SshFs> {
        self.focused_view().read(cx).remote_fs(cx)
    }

    /// 当前格子所在远端目录（远端 shell 上报的 OSC 7）。
    ///
    /// 「文件管理器跟着终端走」就看它（见 [`crate::sidebar_panel::files::FilesState`]），
    /// 所以只问**聚焦**那一格：远端 shell 一 `cd`，面板就跟着换目录。
    pub(crate) fn remote_directory(&self, cx: &App) -> Option<String> {
        self.focused_view().read(cx).remote_directory(cx)
    }

    /// 当前格子所属的连接实例（远端会话才有）。
    pub(crate) fn remote_session(&self, cx: &App) -> Option<SessionId> {
        self.remote_fs(cx).map(|fs| fs.session_id())
    }

    /// 当前聚焦格子的实体 id（活动上下文快照的去重键）。
    pub(crate) fn focused_id(&self) -> EntityId {
        self.focused_view().entity_id()
    }

    /// 当前格子是否已连上。
    pub(crate) fn is_connected(&self, cx: &App) -> bool {
        self.focused_view().read(cx).is_connected(cx)
    }

    /// 把渲染参数推给本页所有格子。
    pub(crate) fn set_render_settings(&mut self, settings: Arc<RenderSettings>, cx: &mut App) {
        let options = crate::shell_integration::launch_options(settings.suggestions.remote_integration);
        for view in &self.panes {
            view.update(cx, |view, cx| {
                view.set_launch_options(options.clone(), cx);
                view.set_render_settings(settings.clone(), cx);
            });
        }
    }

    /// 把焦点同步成「真正持有焦点的那一格」（窗口失焦时保持原值）。
    fn sync_focus(&mut self, window: &Window, cx: &App) {
        if let Some(ix) = self
            .panes
            .iter()
            .position(|view| view.read(cx).is_focused(window))
        {
            self.focused = ix;
        }
    }

    /// 把窗口焦点交给当前格子。
    fn focus_active(&self, window: &mut Window, cx: &mut App) {
        let handle = self.focused_view().read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    /// 在**当前格子**的右 / 下方再开一格并聚焦它。
    pub(crate) fn add_pane(
        &mut self,
        view: Entity<TerminalView>,
        axis: Axis,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index = self.panes.len();
        let node = self.next_node;
        if !self.layout.split(self.focused, axis, index, node) {
            return;
        }
        self.next_node += 1;
        let sub = Self::observe_pane(&view, window, cx);
        self.panes.push(view);
        self.subs.push(sub);
        self.focused = index;
        self.focus_active(window, cx);
        cx.notify();
    }

    /// 关掉第 `ix` 格；返回 `false` 表示这是最后一格（调用方要关掉整个标签页）。
    pub(crate) fn close_pane(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.panes.len() <= 1 || ix >= self.panes.len() {
            return false;
        }
        self.panes.remove(ix);
        let _dropped_sub = self.subs.remove(ix);
        self.layout.remove(ix);
        // 焦点跟着让位后的下标走（被关掉那格的位置由它的邻居顶上）。
        if self.focused >= self.panes.len() {
            self.focused = self.panes.len() - 1;
        } else if self.focused > ix {
            self.focused -= 1;
        }
        self.focus_active(window, cx);
        cx.notify();
        true
    }

    /// 焦点往 `dir` 挪一格；没有邻居就什么也不做。
    pub(crate) fn focus_neighbor(
        &mut self,
        dir: Dir,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync_focus(window, cx);
        let Some(ix) = self.layout.neighbor(self.focused, dir) else {
            return;
        };
        if ix == self.focused {
            return;
        }
        self.focused = ix;
        self.focus_active(window, cx);
        cx.notify();
    }

    /// 递归渲染分屏树：叶子是终端，分叉是「两侧按 `ratio` 摊 + 前缘一条可拖的分隔条」。
    fn render_layout(&self, layout: &Layout, cx: &mut Context<Self>) -> AnyElement {
        match layout {
            Layout::Leaf(ix) => pane_element(&self.panes, *ix),
            Layout::Split {
                node,
                axis,
                ratio,
                first,
                second,
            } => {
                let (node, axis, ratio) = (*node, *axis, *ratio);
                let (first, second) = (
                    self.render_layout(first, cx),
                    self.render_layout(second, cx),
                );
                // 分隔条挂到后一格的前缘（straddle：把手向缝的两侧各伸出一半）。
                let tail = slot(axis, 1. - ratio)
                    .relative()
                    .child(second)
                    .child(
                        resize_handle(("terminal-divider", node), axis).on_drag(
                            DividerDrag { node },
                            |drag, _, _, cx| {
                                // 拖分隔条不该顺带点到底下的终端（聚焦 / 选择）。
                                cx.stop_propagation();
                                cx.new(|_| drag.clone())
                            },
                        ),
                    );
                let container = match axis {
                    Axis::Horizontal => h_flex().size_full().min_w_0(),
                    Axis::Vertical => v_flex().size_full().min_h_0(),
                };
                container
                    .id(("terminal-split", node))
                    .child(slot(axis, ratio).child(first))
                    .child(tail)
                    .on_drag_move::<DividerDrag>(cx.listener(
                        // ⚠️ 嵌套分屏时，指针路径上的**每一层**容器都会收到同一次拖拽：
                        // 只有「分叉 id 和自己对得上」的那层才该动（否则会拿错 bounds）。
                        move |this, drag: &DragMoveEvent<DividerDrag>, _, cx| {
                            this.handle_divider_drag(node, drag, cx)
                        },
                    ))
                    .into_any_element()
            }
        }
    }

    /// 拖分隔条：按指针在**分叉容器**里的位置改那一叉的比例。
    ///
    /// `container` = 挂这个监听的分叉 id（见 `render_layout` 里的说明）。
    fn handle_divider_drag(
        &mut self,
        container: u64,
        drag: &DragMoveEvent<DividerDrag>,
        cx: &mut Context<Self>,
    ) {
        let node = drag.drag(cx).node;
        if node != container {
            return;
        }
        let Some(axis) = self.layout.axis_of(node) else {
            return;
        };
        // 监听挂在分叉容器上 ⇒ `drag.bounds` 就是它的 bounds，指针位置据此换算成比例。
        let (origin, extent, along) = match axis {
            Axis::Horizontal => (
                drag.bounds.left(),
                drag.bounds.size.width,
                drag.event.position.x,
            ),
            Axis::Vertical => (
                drag.bounds.top(),
                drag.bounds.size.height,
                drag.event.position.y,
            ),
        };
        let ratio = ratio_at(origin.as_f32(), extent.as_f32(), along.as_f32());
        if self.layout.set_ratio(node, ratio) {
            cx.notify();
        }
    }
}

/// 分屏里的一格：主轴按 `ratio` 占份，交叉轴铺满（比例是百分比，父容器变宽变窄都跟得上）。
fn slot(axis: Axis, ratio: f32) -> Div {
    let slot = div()
        .flex_basis(relative(ratio))
        .flex_grow(0.)
        .flex_shrink(1.)
        .min_w_0()
        .min_h_0();
    match axis {
        Axis::Horizontal => slot.h_full(),
        Axis::Vertical => slot.w_full(),
    }
}

/// 一块格子。
///
/// ⚠️ 外层 div 必须带 **每个终端唯一** 的 id：`TerminalElement` 自己不贡献 element id
/// （`Interactivity` 是 `default()`），同一页里两块终端会算出同一条 global id 路径。
fn pane_element(panes: &[Entity<TerminalView>], ix: usize) -> AnyElement {
    let Some(view) = panes.get(ix) else {
        return div().size_full().into_any_element();
    };
    div()
        .id(("terminal-pane", view.entity_id()))
        .size_full()
        .min_w_0()
        .min_h_0()
        .overflow_hidden()
        .child(view.clone())
        .into_any_element()
}

/// 拖分隔条时跟着指针走的载荷（gpui 靠载荷的**类型**区分不同的拖拽）。
#[derive(Clone)]
struct DividerDrag {
    node: u64,
}

/// 分隔条的拖拽预览：什么也不画（比例是拖的时候直接改的，不需要浮层）。
impl Render for DividerDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

impl BasePanel for TabPane {
    fn panel_name(&self) -> &'static str {
        "terminal-tab"
    }

    fn closable(&self, _: &App) -> bool {
        true
    }

    fn zoomable(&self, _: &App) -> bool {
        // 页内分屏之后，dock 级的「撑满标签组」已经没有意义。
        false
    }

    fn on_added_to(&mut self, group: WeakEntity<TabGroup>, _: &mut Window, _: &mut Context<Self>) {
        self.group = Some(group);
    }

    /// 切到本标签页：聚焦它当前那一格，并回写「当前标签页」（经 [`AppRoot::defer_after_update`]）。
    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !active {
            return;
        }
        self.focus_active(window, cx);
        let pane = cx.entity().entity_id();
        AppRoot::defer_after_update(self.root.clone(), cx, move |root, window, cx| {
            root.set_active_pane(pane, cx);
            root.sync_active_context(window, cx);
        });
    }
}

impl Panel for TabPane {
    /// 标签上的文字（当前格子的标题）。
    fn tab_name(&self, cx: &App) -> Option<SharedString> {
        Some(self.name(cx))
    }

    fn title_bar(&self, _: &App) -> bool {
        false
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }

    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        None
    }

    /// 工具栏按钮：新建本地终端（新开一个标签页）。
    fn toolbar_buttons(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<Vec<Button>> {
        let root = self.root.clone();
        let group = self.group.clone();
        Some(vec![
            Button::new("new-terminal")
                .icon(IconName::Plus)
                .tooltip("新建本地终端")
                .on_click(move |_, window, cx| {
                    // 先记下本组，再走 `NewLocalTerminal`。
                    let group = group.clone();
                    let root = root.clone();
                    let _ = root.update(cx, |root, _| root.set_pending_session_group(group));
                    window.dispatch_action(Box::new(NewLocalTerminal), cx);
                }),
        ])
    }
}

impl EventEmitter<PanelEvent> for TabPane {}

impl Focusable for TabPane {
    /// 焦点始终归当前那一格。
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.panes.get(self.focused).or_else(|| self.panes.first()) {
            Some(view) => view.read(cx).focus_handle(cx),
            None => self.fallback_focus.clone(),
        }
    }
}

impl Render for TabPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_focus(window, cx);
        div()
            .size_full()
            .overflow_hidden()
            .child(self.render_layout(&self.layout, cx))
    }
}
