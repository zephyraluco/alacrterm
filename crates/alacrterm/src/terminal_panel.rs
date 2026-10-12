//! 终端会话：生命周期（新建 / 激活 / 关闭）与它们在 dock 里的面板 [`SessionPane`]；
//! 会话表 [`AppRoot::terminals`] 与 dock 面板一一对应、顺序一致。入口 [`AppRoot::spawn_terminal`] /
//! [`AppRoot::spawn_session`]。

use std::sync::Arc;

use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, WeakEntity, Window, div,
};
use gpui_kit::component::{
    button::Button,
    dock::{
        BasePanel, DockPlacement, InsertTarget, Panel, PanelControl, PanelEvent, PanelId, TabGroup,
        panel_handle,
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
        false
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
