//! 终端会话：生命周期（新建 / 激活 / 关闭）+ 它们在 dock 里的面板 [`SessionPane`]。
//!
//! 一个会话 = dock 的 `center` 里一块面板，标签栏是自绘的（[`crate::tab_bar`]）；
//! 中间列 = `v_flex[dock, 公共状态栏]`。本模块是**终端实例**的单一入口：新建 / 激活 /
//! 关闭都集中在这里，标签栏的 `+`、欢迎页与侧边栏记录的「打开」都经由它，
//! 保证会话表（[`AppRoot::terminals`]）与 dock 面板一一对应、顺序一致。
//!
//! 两条入口的区别（详见 [`crate::main`] 模块文档）：
//! - [`AppRoot::spawn_terminal`]：直接开一个**本地**终端（标签栏 `+` / 欢迎页）；
//! - [`AppRoot::spawn_session`]：按给定参数开终端（双击侧边栏的会话记录）。
//! 两者都会新建终端实例；侧边栏**添加记录**不走本模块（[`SessionsState::add_record`](crate::sidebar_panel::sessions::SessionsState::add_record)）。
//!
//! 会话进程结束时终端不消失（网格与标签保留，状态栏显示「已断开」）；全部关掉后
//! 中间列显示欢迎页（[`AppRoot::render_welcome`]）。
//!
//! ⚠️ 面板注册进布局必须走 [`panel_handle`](gpui_kit::component::dock::panel_handle)：
//! 裸 `Entity<P>` 会让皮肤取不到表现层 trait，标签退化成只写 `panel_name` 的标题栏。

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
use terminal_view::{RenderSettings, TerminalView};
use util::shell::Shell;

use crate::actions::NewLocalTerminal;
use crate::assets::IconName;
use crate::config;
use crate::AppRoot;

/// 会话的连接目标：决定状态栏「连接状态」一栏显示什么。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SessionTarget {
    /// 本地系统 shell。
    Local,
    /// 通过 ssh 连接的远端主机。
    Ssh {
        user: String,
        host: String,
        port: String,
    },
}

impl SessionTarget {
    /// 状态栏显示用的简短描述。
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Local => "本地".to_string(),
            Self::Ssh { user, host, port } => {
                if user.is_empty() {
                    format!("SSH {host}:{port}")
                } else {
                    format!("SSH {user}@{host}:{port}")
                }
            }
        }
    }
}

/// 新建会话所需的参数（显示名 + 要启动的 shell + 连接目标）。
///
/// 两个来源：标签栏 `+` / 欢迎页的本地终端（[`AppRoot::spawn_terminal`]），
/// 以及双击侧边栏记录（[`crate::sidebar_panel::sessions::SessionRecord::request`]）。
pub(crate) struct SessionRequest {
    /// 用户填写的显示名；`None` 表示回退到终端自身标题。
    pub(crate) name: Option<SharedString>,
    /// 要启动的 shell（本地系统 shell，或 `ssh` 等外部命令）。
    pub(crate) shell: Shell,
    /// 连接目标（用于状态栏展示）。
    pub(crate) target: SessionTarget,
}

/// 一个终端会话：终端视图 + 它在 dock 里的面板。
///
/// 显示名与连接目标放在面板上（标签栏要读名字，状态栏要读连接目标，放一处不会不同步）；
/// 用户命名的连接名优先于终端自己上报的 OSC 标题。
///
/// ⚠️ 与侧边栏的会话**记录**（[`crate::sidebar_panel::sessions::SessionRecord`]）是两条
/// 独立的线：记录只是连接参数，同一条记录可以开任意多个本结构，关掉也不影响记录。
pub(crate) struct Session {
    pub(crate) view: Entity<TerminalView>,
    pub(crate) pane: Entity<SessionPane>,
}

impl Session {
    /// 会话显示名：优先用户命名，否则用终端标题（无标题时为「终端」）。
    pub(crate) fn title(&self, cx: &App) -> SharedString {
        self.pane.read(cx).name(cx)
    }

    /// 连接目标（状态栏「连接」一栏用）。
    pub(crate) fn target<'a>(&self, cx: &'a App) -> &'a SessionTarget {
        self.pane.read(cx).target()
    }
}

impl AppRoot {
    /// 新建一个**本地**终端会话（标签栏 `+` / 欢迎页的「新建终端」）。
    ///
    /// 远端会话不从这里进：先用侧边栏状态栏的 `+` 建一条记录，再双击它
    /// （[`AppRoot::open_session_record`]）。
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

    /// 按给定参数新建会话（PTY 在后台启动），并挂成 dock 里的一块面板。
    ///
    /// `request` 来自一条会话记录（[`crate::sidebar_panel::sessions::SessionRecord::request`]）。
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
        let SessionRequest {
            name,
            shell,
            target,
        } = request;
        let settings = Arc::new(config::settings(cx).render.clone());
        let view = cx.new(|cx| TerminalView::new(None, shell, settings, window, cx));
        // 标题 / 连接状态变化都要刷新界面（标签名、侧边栏、状态栏）。
        cx.observe(&view, |_, _, cx| cx.notify()).detach();

        let root = cx.weak_entity();
        let pane = cx.new(|_| SessionPane::new(view.clone(), name, target, root));
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
        // 新面板由 dock 选中（`set_active` 会把焦点交给终端）；记一次账，再按 dock
        // 的顺序排一遍（分屏时新面板未必在末尾，而侧边栏按这张表的顺序显示）。
        self.active = self.terminals.len() - 1;
        self.sync_sessions_with_dock(cx);
        cx.notify();
    }

    /// 把新的终端渲染参数推给所有存活会话。
    ///
    /// 全局 [`config::Settings`] 与落盘由调用方（设置窗口）负责，这里只管渲染。
    pub(crate) fn apply_render_settings(&mut self, settings: RenderSettings, cx: &mut App) {
        let settings = Arc::new(settings);
        for session in &self.terminals {
            session
                .view
                .update(cx, |view, cx| view.set_render_settings(settings.clone(), cx));
        }
    }

    /// dock 面板报告「我成为当前会话」时同步下标（[`SessionPane::set_active`] 调用），
    /// 按实体 id 反查；查不到（会话刚被关掉）就忽略。
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

    /// 关闭一个终端会话（按下标关闭的入口，如侧边栏右键菜单）。
    ///
    /// 面板移除后 `TerminalView` 一并销毁，`Terminal` 的 Drop 会关闭 PTY 并终止子进程。
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
        // 先让 dock 摘掉面板，再动会话表；关闭最后一刻可能触发别的面板 `set_active`，
        // 那只影响焦点与 `active` 记账。
        self.dock
            .update(cx, |area, cx| area.remove_panel(pane, window, cx));
        self.terminals.remove(index);
        // 选中下标调整逻辑与原先一致（末位用 saturating 防止 usize 下溢）。
        if self.active >= index && self.active > 0 {
            self.active -= 1;
        }
        if self.active >= self.terminals.len() {
            self.active = self.terminals.len().saturating_sub(1);
        }
        // 下标调整后只保证不越界；顺序也跟着 dock 走（关掉的可能不是末位）。
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

    /// 把会话表同步成 dock 的样子：顺序 = dock 里面板的先后，成员 = dock 里还在的面板。
    ///
    /// 由 [`DockEvent::LayoutChanged`](gpui_kit::component::dock::DockEvent) 触发
    /// （见 `AppRoot::new`）。dock 也可能自己关面板（换回 gpui-kit 皮肤时），那种关闭
    /// 不经过本模块，所以这里要负责把不在 dock 里的会话丢掉（`Terminal` 的 Drop 关 PTY）。
    pub(crate) fn sync_sessions_with_dock(&mut self, cx: &mut Context<Self>) {
        let Some(tree) = self.dock.read(cx).layout(DockPlacement::Center) else {
            return;
        };
        // 按 dock 的顺序挑会话：面板顺序也就是视觉顺序（含分屏后的两组）。
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
        // dock 里已经没有的会话落在这里清理（dock 自己关掉的）。
        let dropped = !self.terminals.is_empty();
        let before: Vec<gpui::EntityId> = self.terminals.iter().map(|s| s.pane.entity_id()).collect();
        self.terminals = ordered;
        let after: Vec<gpui::EntityId> = self.terminals.iter().map(|s| s.pane.entity_id()).collect();
        if !dropped && before == after {
            return;
        }
        // 当前会话按面板找回下标（找不到说明刚被关掉，按老规矩往前靠）。
        self.active = active_pane
            .and_then(|pane| self.terminals.iter().position(|s| s.pane.entity_id() == pane))
            .unwrap_or_else(|| self.active.min(self.terminals.len().saturating_sub(1)));
        cx.notify();
    }
}

/// 一个终端会话对应的 dock 面板：会话的展示信息（名字 / 连接目标）+ 终端视图。
///
/// 会话列表本身在 [`AppRoot::terminals`] 上（顺序 = dock 里标签的顺序）。
pub(crate) struct SessionPane {
    view: Entity<TerminalView>,
    root: WeakEntity<AppRoot>,
    /// 新建会话时用户填的名字；`None` = 回退到终端自己上报的标题。
    name: Option<SharedString>,
    target: SessionTarget,
    /// 本面板所在的标签组（`on_added_to` 告知）：「新建终端」靠它判断新会话进哪一组。
    group: Option<WeakEntity<TabGroup>>,
}

impl SessionPane {
    pub(crate) fn new(
        view: Entity<TerminalView>,
        name: Option<SharedString>,
        target: SessionTarget,
        root: WeakEntity<AppRoot>,
    ) -> Self {
        Self {
            view,
            root,
            name,
            target,
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

    /// 连接目标（状态栏「连接」一栏用）。
    pub(crate) fn target(&self) -> &SessionTarget {
        &self.target
    }
}

impl BasePanel for SessionPane {
    fn panel_name(&self) -> &'static str {
        "terminal-session"
    }

    /// 面板可被关闭（自绘标签栏的 `×` 走应用层，这里保持一致）。
    fn closable(&self, _: &App) -> bool {
        true
    }

    /// 不做 dock 的「最大化单面板」：会绕过中间列的状态栏。
    fn zoomable(&self, _: &App) -> bool {
        false
    }

    /// 记下自己所在的标签组。
    fn on_added_to(&mut self, group: WeakEntity<TabGroup>, _: &mut Window, _: &mut Context<Self>) {
        self.group = Some(group);
    }

    /// 切到本会话：把键盘焦点交给它的终端，并让根视图记下「当前会话」。
    ///
    /// ⚠️ 同步信息一律走 [`AppRoot::defer_after_update`]：这里可能发生在根视图自己的
    /// 更新过程中（新建 / 关闭会话都会触发 dock 重新选面板），同步改会重入。
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

    /// 不要内边距：终端背景要铺满标签栏底边线与状态栏之间。
    fn inner_padding(&self, _: &App) -> bool {
        false
    }

    /// 不做单面板最大化（同 `zoomable`）。
    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        None
    }

    /// 标签栏右端的工具栏按钮：新建**本地**终端（关闭按钮在每个标签上，见 [`crate::tab_bar`]）。
    ///
    /// 标签栏的 `+` 只负责「快速开一个本地终端」，**不弹任何对话框**；要连远端请先用
    /// 侧边栏「会话」栏底部状态栏右下角的 `+` 建一条记录，再双击那条记录
    /// （[`AppRoot::open_session_record`]）。
    ///
    /// 工具栏区只画当前组当前面板的按钮，所以「新建」得先把「本组」记到根视图上，
    /// 否则新会话会被 `add_panel_view` 塞进 center 的第一个标签组。
    fn toolbar_buttons(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<Vec<Button>> {
        let root = self.root.clone();
        let group = self.group.clone();
        Some(vec![
            Button::new("new-terminal")
                .icon(IconName::Plus)
                .tooltip("新建本地终端")
                .on_click(move |_, window, cx| {
                    // 先告诉根视图「新会话进这一组」，再走统一的 `NewLocalTerminal` 入口。
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
        // 面板内容就是终端本体（`TerminalView` 是 `size_full`，dock 会给它确定尺寸）。
        div().size_full().child(self.view.clone())
    }
}
