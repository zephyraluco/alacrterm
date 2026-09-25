//! 终端会话：生命周期（新建 / 激活 / 关闭）+ 它们在 dock 里的面板 [`SessionPane`]。
//!
//! 一个会话 = dock 的 `center` 里一块面板（标签栏见 [`crate::tab_bar`]）；会话表
//! [`AppRoot::terminals`] 与 dock 面板一一对应、顺序一致。入口：[`AppRoot::spawn_terminal`]
//! （直接开本地终端）与 [`AppRoot::spawn_session`]（按参数开，如双击侧边栏记录）；
//! 会话进程结束后面板保留（状态栏显示「已断开」），全部关掉则中间列显示欢迎页。
//!
//! ⚠️ 面板注册进布局必须走 [`panel_handle`](gpui_kit::component::dock::panel_handle)。

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

/// 新建会话所需的参数：显示名 + 要连接到哪儿（见 [`TerminalTarget`]）。
///
/// `Clone`：对话框的构建闭包是 `Fn`、每帧都会被调用，里面的请求得能复制。
#[derive(Clone)]
pub(crate) struct SessionRequest {
    /// 用户填写的显示名；`None` 表示回退到终端自身标题。
    pub(crate) name: Option<SharedString>,
    /// 要连接的目标：本地 shell 走 PTY，SSH 走 russh（见 [`TerminalTarget`]）。
    pub(crate) target: TerminalTarget,
}

/// 一个终端会话：终端视图 + 它在 dock 里的面板（显示名与「是不是远端」都在面板上）。
///
/// 与侧边栏的会话**记录**（[`crate::sidebar_panel::sessions::SessionRecord`]）无关：
/// 同一条记录可以开任意多个会话。
pub(crate) struct Session {
    pub(crate) view: Entity<TerminalView>,
    pub(crate) pane: Entity<SessionPane>,
}

impl Session {
    /// 是不是远端（SSH）会话 —— 「文件管理器」只服务远端会话。
    pub(crate) fn is_remote(&self, cx: &App) -> bool {
        self.pane.read(cx).is_remote()
    }
}

impl AppRoot {
    /// 新建一个**本地**终端会话（标签栏 `+` / 欢迎页的「新建终端」）。
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

    /// 按给定参数新建会话（PTY 在后台启动），并挂成 dock 里的一块面板。
    pub(crate) fn spawn_session(
        &mut self,
        request: SessionRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 新会话进哪一组：`+` 按钮留下的提示优先，否则跟当前会话同组。
        let group = self
            .pending_session_group
            .take()
            .or_else(|| self.session_group(self.active, cx));
        self.spawn_session_in(request, group, window, cx);
    }

    /// 某个会话所在的标签组。
    fn session_group(&self, index: usize, cx: &App) -> Option<WeakEntity<TabGroup>> {
        self.terminals
            .get(index)
            .and_then(|session| session.pane.read(cx).group())
    }

    /// [`Self::spawn_session`] 的实现：`group` 指定新面板放进哪个标签组。
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
        // 主机密钥确认（首次连接）由应用弹窗；本地会话用不到（见 [`host_key`]）。
        let handler = remote.then(|| host_key::host_key_prompt_handler(root.clone()));
        let view = cx
            .new(|cx| TerminalView::new(None, target, handler, settings, window, cx));
        // 标题 / 连接状态变化都要刷新界面（标签名、侧边栏、文件管理器）。
        cx.observe(&view, |_, _, cx| cx.notify()).detach();

        let pane = cx.new(|_| SessionPane::new(view.clone(), name, remote, root));
        let handle = panel_handle(pane.clone());
        self.dock.update(cx, |area, cx| {
            area.add_panel_view(handle, DockPlacement::Center, None, window, cx);
        });

        // `add_panel_view` 只塞进 center 的**第一个**标签组，分屏时要挪到目标组。
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
        // 记一次账，再按 dock 的顺序排一遍（分屏时新面板未必在末尾）。
        self.active = self.terminals.len() - 1;
        self.sync_sessions_with_dock(cx);
        cx.notify();
    }

    /// 把新的终端渲染参数推给所有存活会话（全局配置与落盘由调用方负责）。
    pub(crate) fn apply_render_settings(&mut self, settings: RenderSettings, cx: &mut App) {
        let settings = Arc::new(settings);
        for session in &self.terminals {
            session
                .view
                .update(cx, |view, cx| view.set_render_settings(settings.clone(), cx));
        }
    }

    /// dock 面板成为当前会话时按实体 id 同步下标（查不到就忽略）。
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

    /// 关闭一个终端会话（按下标，如侧边栏右键菜单）。
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
        // 先让 dock 摘掉面板，再动会话表。
        self.dock
            .update(cx, |area, cx| area.remove_panel(pane, window, cx));
        self.terminals.remove(index);
        // 末位用 saturating 防止 usize 下溢。
        if self.active >= index && self.active > 0 {
            self.active -= 1;
        }
        if self.active >= self.terminals.len() {
            self.active = self.terminals.len().saturating_sub(1);
        }
        // 顺序跟着 dock 走（关掉的可能不是末位）。
        self.sync_sessions_with_dock(cx);
        cx.notify();
    }

    /// 按面板 id 关闭会话（标签栏上的 `×` / 中键：按钮属于哪块面板就关哪块）。
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

    /// 把会话表同步成 dock 的样子：顺序 = dock 里面板的先后，成员 = dock 里还在的面板
    /// （dock 自己关掉的面板也在这里被丢掉）。由 `DockEvent::LayoutChanged` 触发。
    pub(crate) fn sync_sessions_with_dock(&mut self, cx: &mut Context<Self>) {
        let Some(tree) = self.dock.read(cx).layout(DockPlacement::Center) else {
            return;
        };
        // 按 dock 的顺序挑会话。
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
        // 剩下的是 dock 里已经没有的会话（dock 自己关掉的）。
        let dropped = !self.terminals.is_empty();
        let before: Vec<gpui::EntityId> = self.terminals.iter().map(|s| s.pane.entity_id()).collect();
        self.terminals = ordered;
        let after: Vec<gpui::EntityId> = self.terminals.iter().map(|s| s.pane.entity_id()).collect();
        if !dropped && before == after {
            return;
        }
        // 当前会话按面板找回下标（找不到就往前靠）。
        self.active = active_pane
            .and_then(|pane| self.terminals.iter().position(|s| s.pane.entity_id() == pane))
            .unwrap_or_else(|| self.active.min(self.terminals.len().saturating_sub(1)));
        cx.notify();
    }
}

/// 一个终端会话对应的 dock 面板：展示信息（名字 / 是不是远端）+ 终端视图。
pub(crate) struct SessionPane {
    view: Entity<TerminalView>,
    root: WeakEntity<AppRoot>,
    /// 新建会话时用户填的名字；`None` = 回退到终端自己上报的标题。
    name: Option<SharedString>,
    /// 是不是远端（SSH）会话 —— 「文件管理器」只服务远端会话（见 [`crate::sidebar_panel::files`]）。
    remote: bool,
    /// 本面板所在的标签组（`on_added_to` 告知）：「新建终端」靠它判断新会话进哪一组。
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

    /// 本面板当前所在的标签组。
    pub(crate) fn group(&self) -> Option<WeakEntity<TabGroup>> {
        self.group.clone()
    }

    /// 会话显示名：优先用户命名，否则终端标题。
    pub(crate) fn name(&self, cx: &App) -> SharedString {
        self.name
            .clone()
            .unwrap_or_else(|| self.view.read(cx).title())
    }

    /// 是不是远端（SSH）会话。
    pub(crate) fn is_remote(&self) -> bool {
        self.remote
    }
}

impl BasePanel for SessionPane {
    fn panel_name(&self) -> &'static str {
        "terminal-session"
    }

    /// 面板可被关闭。
    fn closable(&self, _: &App) -> bool {
        true
    }

    /// 不做 dock 的「最大化单面板」。
    fn zoomable(&self, _: &App) -> bool {
        false
    }

    /// 记下自己所在的标签组。
    fn on_added_to(&mut self, group: WeakEntity<TabGroup>, _: &mut Window, _: &mut Context<Self>) {
        self.group = Some(group);
    }

    /// 切到本会话：把键盘焦点交给它的终端，并让根视图记下「当前会话」。
    ///
    /// ⚠️ 回写根视图要经 [`AppRoot::defer_after_update`]（这里可能正处在根视图的更新过程中）。
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
    /// 标签上的文字（自绘标签栏取它当标题）。
    fn tab_name(&self, cx: &App) -> Option<SharedString> {
        Some(self.name(cx))
    }

    /// 标签栏由 dock 提供，不要额外的面板标题栏。
    fn title_bar(&self, _: &App) -> bool {
        false
    }

    /// 不要内边距：终端背景要铺满。
    fn inner_padding(&self, _: &App) -> bool {
        false
    }

    /// 不做单面板最大化（同 `zoomable`）。
    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        None
    }

    /// 标签栏右端的工具栏按钮：新建本地终端（关闭按钮在每个标签上，见 [`crate::tab_bar`]）。
    ///
    /// 先把「本组」记到根视图上，否则新会话会被 `add_panel_view` 塞进 center 的第一个标签组。
    fn toolbar_buttons(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<Vec<Button>> {
        let root = self.root.clone();
        let group = self.group.clone();
        Some(vec![
            Button::new("new-terminal")
                .icon(IconName::Plus)
                .tooltip("新建本地终端")
                .on_click(move |_, window, cx| {
                    // 先告诉根视图「新会话进这一组」，再走统一的 `NewLocalTerminal`。
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
    /// 焦点始终归终端的视图（面板自己不抢焦点）。
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.view.read(cx).focus_handle(cx)
    }
}

impl Render for SessionPane {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // 面板内容就是终端本体（dock 会给它确定尺寸）。
        div().size_full().child(self.view.clone())
    }
}
