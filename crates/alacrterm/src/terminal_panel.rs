//! 终端会话：生命周期（新建 / 激活 / 关闭）与它们在 dock 里的面板 [`SessionPane`]；
//! 会话表 [`AppRoot::terminals`] 与 dock 面板一一对应、顺序一致。入口 [`AppRoot::spawn_terminal`] /
//! [`AppRoot::spawn_session`]。

use std::sync::Arc;

use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, WeakEntity, Window, div,
};
use gpui_kit::base::Placement;
use gpui_kit::component::{
    button::Button,
    dock::{
        BasePanel, DockPlacement, InsertTarget, NodeId, Panel, PanelControl, PanelEvent, PanelId,
        TabGroup, panel_handle,
    },
};
use terminal::TerminalTarget;
use terminal_view::{RenderSettings, TerminalView};

use crate::actions::NewLocalTerminal;
use crate::assets::IconName;
use crate::config;
use crate::dialog::host_key;
use crate::AppRoot;

/// 新建会话的参数。
#[derive(Clone)]
pub(crate) struct SessionRequest {
    /// 显示名；`None` = 用终端标题。
    pub(crate) name: Option<SharedString>,
    /// 连接目标（见 [`TerminalTarget`]）。
    pub(crate) target: TerminalTarget,
}

/// 一个终端会话：终端视图 + dock 面板。
pub(crate) struct Session {
    pub(crate) view: Entity<TerminalView>,
    pub(crate) pane: Entity<SessionPane>,
}

impl Session {
    pub(crate) fn is_remote(&self, cx: &App) -> bool {
        self.pane.read(cx).is_remote()
    }
}

impl AppRoot {
    /// 新建本地终端会话。
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

    /// 按参数新建会话并挂成 dock 面板。
    pub(crate) fn spawn_session(
        &mut self,
        request: SessionRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 新会话进哪一组。
        let group = self
            .pending_session_group
            .take()
            .or_else(|| self.session_group(self.active, cx));
        self.spawn_session_in(request, group, window, cx);
    }

    fn session_group(&self, index: usize, cx: &App) -> Option<WeakEntity<TabGroup>> {
        self.terminals
            .get(index)
            .and_then(|session| session.pane.read(cx).group())
    }

    /// [`Self::spawn_session`] 的实现（`group` = 目标标签组）。
    fn spawn_session_in(
        &mut self,
        request: SessionRequest,
        group: Option<WeakEntity<TabGroup>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let settings = Arc::new(config::settings(cx).render.clone());
        let SessionRequest { name, target } = request;
        let remote = target.is_ssh();
        let root = cx.weak_entity();
        // 主机密钥确认弹窗（仅远端）。
        let handler = remote.then(|| host_key::host_key_prompt_handler(root.clone()));
        let view = cx
            .new(|cx| TerminalView::new(None, target, handler, settings, window, cx));
        // 标题 / 连接状态变化时重绘。
        cx.observe(&view, |_, _, cx| cx.notify()).detach();

        let pane = cx.new(|_| SessionPane::new(view.clone(), name, remote, root));
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

        self.terminals.push(Session { view, pane });
        // 按 dock 的顺序重排。
        self.active = self.terminals.len() - 1;
        self.sync_sessions_with_dock(cx);
        cx.notify();
    }

    /// 把渲染参数推给所有存活会话。
    pub(crate) fn apply_render_settings(&mut self, settings: RenderSettings, cx: &mut App) {
        let settings = Arc::new(settings);
        for session in &self.terminals {
            session
                .view
                .update(cx, |view, cx| view.set_render_settings(settings.clone(), cx));
        }
    }

    /// 按面板 id 同步当前会话下标。
    pub(crate) fn set_active_pane(&mut self, pane: gpui::EntityId, cx: &mut Context<Self>) {
        let Some(index) = self
            .terminals
            .iter()
            .position(|session| session.pane.entity_id() == pane)
        else {
            return;
        };
        if self.active == index {
            return;
        }
        self.active = index;
        cx.notify();
    }

    /// 关闭一个终端会话。
    pub(crate) fn close_terminal(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.terminals.get(index) else {
            return;
        };
        let pane = session.pane.clone();
        // 先让 dock 摘掉面板。
        self.dock
            .update(cx, |area, cx| area.remove_panel(pane, window, cx));
        self.terminals.remove(index);
        if self.active >= index && self.active > 0 {
            self.active -= 1;
        }
        if self.active >= self.terminals.len() {
            self.active = self.terminals.len().saturating_sub(1);
        }
        // 顺序跟着 dock 走。
        self.sync_sessions_with_dock(cx);
        cx.notify();
    }

    /// 按面板 id 关闭会话。
    pub(crate) fn close_panel_id(
        &mut self,
        panel: PanelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self
            .terminals
            .iter()
            .position(|session| panel_handle(session.pane.clone()).panel_id(cx) == panel)
        else {
            return;
        };
        self.close_terminal(index, window, cx);
    }

    /// 把会话表同步成 dock 的样子（顺序 / 成员）。
    pub(crate) fn sync_sessions_with_dock(&mut self, cx: &mut Context<Self>) {
        let Some(tree) = self.dock.read(cx).layout(DockPlacement::Center) else {
            return;
        };
        let order: Vec<PanelId> = tree.panels().collect();
        let active_pane = self.terminals.get(self.active).map(|s| s.pane.entity_id());
        let mut ordered = Vec::with_capacity(self.terminals.len());
        for id in order {
            let Some(ix) = self
                .terminals
                .iter()
                .position(|session| panel_handle(session.pane.clone()).panel_id(cx) == id)
            else {
                continue;
            };
            ordered.push(self.terminals.remove(ix));
        }
        let dropped = !self.terminals.is_empty();
        let before: Vec<gpui::EntityId> = self.terminals.iter().map(|s| s.pane.entity_id()).collect();
        self.terminals = ordered;
        let after: Vec<gpui::EntityId> = self.terminals.iter().map(|s| s.pane.entity_id()).collect();
        if !dropped && before == after {
            return;
        }
        // 当前会话按面板找回下标。
        self.active = active_pane
            .and_then(|pane| self.terminals.iter().position(|s| s.pane.entity_id() == pane))
            .unwrap_or_else(|| self.active.min(self.terminals.len().saturating_sub(1)));
        cx.notify();
    }

    /// 「当前会话」下标：焦点所在的终端优先，否则用 `active`。
    ///
    /// 键位动作都是从键盘派发来的（焦点必然在某个终端里），但 `Window` 只有拿到窗口的
    /// 回调才有（见 [`AppRoot::defer_after_update`]），所以键位动作一律走这条路径而不用
    /// [`Self::active`] 直觉判断。
    pub(crate) fn active_session_index(&self, window: &Window, cx: &App) -> Option<usize> {
        self.terminals
            .iter()
            .position(|session| session.view.read(cx).is_focused(window))
            .or_else(|| (!self.terminals.is_empty()).then(|| self.active.min(self.terminals.len() - 1)))
    }

    /// 对「当前会话」的终端视图做一件事（找不到就什么也不做）。
    ///
    /// 键位动作的另一条入口：不需要 `Window`，用 `active` 即可（渲染时按焦点更新过）。
    pub(crate) fn with_active_terminal(
        &self,
        cx: &mut App,
        action: impl FnOnce(&mut TerminalView, &mut Context<TerminalView>),
    ) {
        let Some(session) = self.terminals.get(self.active) else {
            return;
        };
        session.view.update(cx, action);
    }

    /// 关闭当前会话（`Ctrl+Shift+W`）。
    pub(crate) fn close_active_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.active_session_index(window, cx) else {
            return;
        };
        self.close_terminal(index, window, cx);
    }

    /// 前后切换会话（`Ctrl+Tab` / `Ctrl+Shift+Tab`），到头折返。
    pub(crate) fn step_session(&mut self, delta: i32, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.terminals.len();
        if count == 0 {
            return;
        }
        let Some(current) = self.active_session_index(window, cx) else {
            return;
        };
        let next = (current as i32 + delta).rem_euclid(count as i32) as usize;
        self.activate_session(next, window, cx);
    }

    /// 跳到第 `index` 个会话：切到它所在的标签页并聚焦。
    pub(crate) fn activate_session(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.terminals.get(index) else {
            return;
        };
        let pane = session.pane.clone();
        let focus = session.view.read(cx).focus_handle(cx);
        let group = session.pane.read(cx).group().and_then(|group| group.upgrade());
        self.active = index;

        let Some(group) = group else {
            // 还没进过标签组（首个会话）：直接聚焦它。
            window.focus(&focus, cx);
            cx.notify();
            return;
        };
        let panel = panel_handle(pane.clone()).panel_id(cx);
        let tab = group
            .read(cx)
            .panels()
            .iter()
            .position(|candidate| candidate.panel_id(cx) == panel);
        match tab {
            // 切标签页会顺带聚焦（`TabGroup::select_tab` → `focus_active_panel`）。
            Some(tab) => group.update(cx, |group, cx| group.select_tab(tab, window, cx)),
            None => window.focus(&focus, cx),
        }
        cx.notify();
    }

    /// 在「当前会话」旁边新建一个会话并放进新的标签组（`Ctrl+Shift+D` / `Ctrl+Shift+S`）。
    pub(crate) fn split_active_session(
        &mut self,
        placement: Placement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(node) = self.active_session_node(window, cx) else {
            return;
        };
        // 新建的会话先按常规进当前组，再挪到旁边新开的组里。
        self.spawn_terminal(window, cx);
        let Some(session) = self.terminals.last() else {
            return;
        };
        let panel = panel_handle(session.pane.clone()).panel_id(cx);
        self.dock.update(cx, |area, cx| {
            area.split_at(node, panel, placement, window, cx)
        });
        self.sync_sessions_with_dock(cx);
        cx.notify();
    }

    /// 最大化 / 还原当前会话所在的标签组（`Ctrl+Shift+Z`）。
    pub(crate) fn toggle_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dock.read(cx).is_zoomed() {
            self.dock
                .update(cx, |area, cx| area.set_zoomed_out(window, cx));
            return;
        }
        let Some(node) = self.active_session_node(window, cx) else {
            return;
        };
        self.dock
            .update(cx, |area, cx| area.set_zoomed_in(node, window, cx));
    }

    /// 当前会话在 dock 布局里所在的节点（标签组），没有就 `None`。
    fn active_session_node(&self, window: &Window, cx: &App) -> Option<NodeId> {
        let index = self.active_session_index(window, cx)?;
        let panel = panel_handle(self.terminals.get(index)?.pane.clone()).panel_id(cx);
        self.dock
            .read(cx)
            .layout(DockPlacement::Center)
            .and_then(|tree| tree.find_panel_node(panel))
    }
}

/// 一个会话对应的 dock 面板。
pub(crate) struct SessionPane {
    view: Entity<TerminalView>,
    root: WeakEntity<AppRoot>,
    /// 用户填的名字；`None` = 用终端标题。
    name: Option<SharedString>,
    remote: bool,
    /// 本面板所在的标签组。
    group: Option<WeakEntity<TabGroup>>,
}

impl SessionPane {
    pub(crate) fn new(
        view: Entity<TerminalView>,
        name: Option<SharedString>,
        remote: bool,
        root: WeakEntity<AppRoot>,
    ) -> Self {
        Self {
            view,
            root,
            name,
            remote,
            group: None,
        }
    }

    pub(crate) fn group(&self) -> Option<WeakEntity<TabGroup>> {
        self.group.clone()
    }

    /// 会话显示名。
    pub(crate) fn name(&self, cx: &App) -> SharedString {
        self.name
            .clone()
            .unwrap_or_else(|| self.view.read(cx).title())
    }

    pub(crate) fn is_remote(&self) -> bool {
        self.remote
    }
}

impl BasePanel for SessionPane {
    fn panel_name(&self) -> &'static str {
        "terminal-session"
    }

    fn closable(&self, _: &App) -> bool {
        true
    }

    fn zoomable(&self, _: &App) -> bool {
        // `Ctrl+Shift+Z`（`alacrterm::ToggleZoom`）要把标签组撑满 dock：
        // gpui-kit 的 `TabGroup::set_zoomed` 会拒绝不可缩放的 panel。
        true
    }

    fn on_added_to(&mut self, group: WeakEntity<TabGroup>, _: &mut Window, _: &mut Context<Self>) {
        self.group = Some(group);
    }

    /// 切到本会话：聚焦它的终端，并回写「当前会话」（经 [`AppRoot::defer_after_update`]）。
    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !active {
            return;
        }
        let focus = self.view.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        let pane = cx.entity().entity_id();
        AppRoot::defer_after_update(self.root.clone(), cx, move |root, _, cx| {
            root.set_active_pane(pane, cx)
        });
    }
}

impl Panel for SessionPane {
    /// 标签上的文字。
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

    /// 工具栏按钮：新建本地终端。
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

impl EventEmitter<PanelEvent> for SessionPane {}

impl Focusable for SessionPane {
    /// 焦点始终归终端视图。
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.view.read(cx).focus_handle(cx)
    }
}

impl Render for SessionPane {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {

        div().size_full().child(self.view.clone())
    }
}
