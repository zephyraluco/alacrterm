//! 左侧可折叠面板，共享会话和文件视图的状态。
//! 视图切换入口位于全局底部状态栏。

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Pixels,
    Render, Styled as _, Subscription, Window, px,
};
use gpui_kit::component::{
    Icon,
    empty::{Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyMediaVariant, EmptyTitle},
    resizable::ResizableState,
};

use crate::AppRoot;
use crate::assets::IconName;
use crate::components::SidebarRow;

mod activity_bar;
pub(crate) mod files;
mod header;
pub(crate) mod sessions;
mod shared;

use files::FilesState;
use sessions::SessionsState;

/// 面板标题栏高度。
pub(super) const PANEL_HEADER_HEIGHT: Pixels = px(28.);
/// 侧边栏默认宽度。
pub(crate) const SIDEBAR_DEFAULT_WIDTH: Pixels = px(220.);
/// 窄到该宽度即折叠。
const SIDEBAR_COLLAPSE_WIDTH: Pixels = px(24.);
/// 侧边栏在分栏组里的下标。
const SIDEBAR_PANEL_INDEX: usize = 0;

/// 拖到窗口左沿（≤ [`SIDEBAR_COLLAPSE_WIDTH`]）时折叠面板。
fn sidebar_width_after_drag(width: Pixels) -> Option<Pixels> {
    (width > SIDEBAR_COLLAPSE_WIDTH).then_some(width)
}

/// 视图内容为空时的占位(gpui-kit [`Empty`])。
pub(super) fn empty_state(
    icon: IconName,
    title: &'static str,
    description: Option<&'static str>,
) -> Empty {
    let header = EmptyHeader::new()
        .media(
            EmptyMedia::new()
                .with_variant(EmptyMediaVariant::Icon)
                .child(Icon::new(icon)),
        )
        .title(EmptyTitle::new().child(title));
    let header = match description {
        Some(text) => header.description(EmptyDescription::new().child(text)),
        None => header,
    };

    Empty::new().flex_none().p_3().header(header)
}

/// 侧边栏的视图。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SidebarView {
    Sessions,
    Files,
}

impl SidebarView {
    /// 全部视图（按摆放顺序）。
    pub(super) const ALL: [SidebarView; 2] = [Self::Sessions, Self::Files];

    /// 这个视图当前是否摆出来。
    fn visible_with(self, files_enabled: bool) -> bool {
        match self {
            Self::Sessions => true,
            Self::Files => files_enabled,
        }
    }

    /// 图标名。
    fn icon(self) -> IconName {
        match self {
            Self::Sessions => IconName::SquareTerminal,
            Self::Files => IconName::Folder,
        }
    }

    /// 标签文字。
    fn label(self) -> &'static str {
        match self {
            Self::Sessions => "会话",
            Self::Files => "文件管理器",
        }
    }

    /// 悬停提示与无障碍名称。
    fn tooltip(self) -> &'static str {
        match self {
            Self::Sessions => "终端会话",
            Self::Files => "当前终端目录下的内容",
        }
    }

    /// 元素 id 用的短名。
    fn id(self) -> &'static str {
        match self {
            Self::Sessions => "sessions",
            Self::Files => "files",
        }
    }
}

/// 最左侧整列（全应用唯一一条）。
pub(crate) struct Sidebar {
    /// 当前显示的视图。
    active: SidebarView,
    /// 「文件管理器」是否可用。
    files_enabled: bool,
    /// 是否显示侧边栏面板。
    visible: bool,
    /// 期望宽度。
    width: Pixels,
    /// 分栏面板共享状态。
    resize: Entity<ResizableState>,
    /// 上次看到的容器宽度。
    split_width: Option<Pixels>,
    /// 「会话」视图状态。
    sessions: Entity<SessionsState>,
    /// 「文件管理器」视图状态。
    files: Entity<FilesState>,
    /// 把两个视图的变化转发成本实体的重绘。
    _sessions_sub: Subscription,
    _files_sub: Subscription,
}

impl Sidebar {
    pub(crate) fn new(
        sessions: Entity<SessionsState>,
        files: Entity<FilesState>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            active: SidebarView::Sessions,
            files_enabled: false,
            visible: false,
            width: SIDEBAR_DEFAULT_WIDTH,
            resize: cx.new(|_| ResizableState::default()),
            split_width: None,
            sessions: sessions.clone(),
            files: files.clone(),
            _sessions_sub: cx.observe(&sessions, |_, _, cx| cx.notify()),
            _files_sub: cx.observe(&files, |_, _, cx| cx.notify()),
        }
    }

    pub(crate) fn render_status_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        activity_bar::status_bar(self, cx)
    }

    pub(crate) fn visible(&self) -> bool {
        self.visible
    }

    fn files_view_available(&self, cx: &App) -> bool {
        self.files_enabled || self.files.read(cx).has_transfers(cx)
    }

    /// 设置是否显示。
    pub(crate) fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        self.on_visibility_change();
        cx.notify();
    }

    /// 同步「文件管理器」的可用性。
    pub(crate) fn set_files_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.files_enabled == enabled {
            return;
        }
        self.files_enabled = enabled;
        cx.notify();
    }

    /// 期望宽度。
    pub(crate) fn width(&self) -> Pixels {
        self.width
    }

    /// 记下拖拽后的面板宽度；拖到左沿则收起。
    pub(crate) fn set_panel_width(&mut self, panel_width: Pixels, cx: &mut Context<Self>) {
        let Some(width) = sidebar_width_after_drag(panel_width) else {
            self.set_visible(false, cx);
            return;
        };
        self.width = width;
    }

    /// 清掉 `split_width`（下次 [`Sidebar::pin_width`] 重钉）。
    fn on_visibility_change(&mut self) {
        self.split_width = None;
    }

    /// 分栏面板共享状态。
    pub(crate) fn resize_state(&self) -> &Entity<ResizableState> {
        &self.resize
    }

    /// 容器宽度变过就把整列钉回期望宽度。
    pub(crate) fn pin_width(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        let width = self.width;
        let container = self.resize.read(cx).container_size();
        if self.split_width != Some(container) {
            self.split_width = Some(container);
            let index = SIDEBAR_PANEL_INDEX;
            self.resize.update(cx, |state, cx| {
                state.resize_panel(index, width, window, cx);
            });
        }
    }

    /// 让本栏可见。
    fn show(&mut self) {
        self.visible = true;
        self.on_visibility_change();
    }

    /// 当前该摆出来的视图；都不可用则 `None`。
    fn active_view(&self, cx: &App) -> Option<SidebarView> {
        let files_available = self.files_view_available(cx);
        if self.active.visible_with(files_available) {
            return Some(self.active);
        }
        SidebarView::ALL
            .into_iter()
            .find(|view| view.visible_with(files_available))
    }

    /// 图标栏点选：显示该视图；再点当前那枚 = 折叠。
    pub(crate) fn toggle_view(&mut self, view: SidebarView, cx: &mut Context<Self>) {
        if self.visible && self.active_view(cx) == Some(view) {
            self.set_visible(false, cx);
            return;
        }
        self.open_view(view, cx);
    }

    /// 打开某个视图。
    pub(crate) fn open_view(&mut self, view: SidebarView, cx: &mut Context<Self>) {
        let was_visible = self.visible;
        let changed = self.active != view;
        self.active = view;
        self.show();
        if changed || !was_visible {
            cx.notify();
        }
    }
}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        activity_bar::panel(self, cx)
    }
}

impl AppRoot {
    /// 显示 / 折叠侧边栏（`Ctrl+Shift+B`）。
    pub(crate) fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_visible(!sidebar.visible(), cx));
    }

    /// 打开侧边栏的某个视图（`Ctrl+Shift+E` / `Ctrl+Shift+G`）。
    pub(crate) fn open_sidebar_view(&mut self, view: SidebarView, cx: &mut Context<Self>) {
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.open_view(view, cx));
    }
}

/// 侧边栏内容：两种视图的行，统一交给 [`crate::components::Sidebar`]。
#[derive(Clone)]
enum SidebarContent {
    Sessions(sessions::SessionsItem),
    Files(files::FilesItem),
}

impl SidebarRow for SidebarContent {
    /// 按需把这一行画出来。
    fn render(self, cx: &mut App) -> AnyElement {
        match self {
            Self::Sessions(item) => item.render(cx),
            Self::Files(item) => item.render(cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SIDEBAR_COLLAPSE_WIDTH, sidebar_width_after_drag};
    use gpui::px;

    #[test]
    fn sidebar_width_has_no_min_or_max() {
        assert_eq!(sidebar_width_after_drag(px(25.)), Some(px(25.)));
        assert_eq!(sidebar_width_after_drag(px(2000.)), Some(px(2000.)));
    }

    #[test]
    fn dragging_to_the_left_edge_collapses() {
        assert_eq!(sidebar_width_after_drag(px(0.)), None);
        assert_eq!(sidebar_width_after_drag(px(-10.)), None);
        assert_eq!(sidebar_width_after_drag(SIDEBAR_COLLAPSE_WIDTH), None);
    }
}
