//! 最左侧的**整列**(VS Code 的「活动栏 + 侧边栏」):48px 的图标栏 + 它右侧的侧边栏面板
//! (顶部面板标题栏 + 视图内容),一直铺到窗口底部那条状态栏的上沿。
//!
//! 整列 = 一个实体([`Sidebar`]),持有当前视图、折叠状态、期望宽度与那一组分栏面板状态,
//! 根视图把它摆进分栏面板。外观由 `components/` 里的组件画
//! ([`crate::components::ActivityBar`] 的图标列 + [`crate::components::Sidebar`] 面板),
//! 本模块负责装配(见 [`activity_bar::left_column`])。
//!
//! 视图内容是两个共享实体:[`sessions::SessionsState`](「会话」)与
//! [`files::FilesState`](「文件管理器」)——同一时刻只摆一个。后者只对远端会话可见,
//! 可用性由根视图每帧同步([`Sidebar::set_files_enabled`])。
//!
//! - **图标栏**([`activity_bar::left_column`]):每个视图一枚图标 + 底部「设置」,常驻不折叠;
//!   点一枚 = 让侧边栏显示这个视图,再点当前那枚 = 折叠侧边栏。
//! - **面板标题栏**([`header`]):当前视图的标题 + 右端「…」菜单(列出各视图、勾选当前那个)。
//! - **「会话」视图的右键菜单**(右键落在空白处 / 面板标题上):顶层「新建会话」
//!   ([`crate::actions::NewSession`])、「新建文件夹」([`crate::actions::NewFolder`]);
//!   条目自己的右键菜单在条目身上(见 [`sessions`])。
//! - **空内容占位**([`empty_state`]):视图没内容时统一摆 gpui-kit 的 `Empty`;
//!   一个可用视图都没有时什么都不摆,只留背景。
//!
//! 整列与中间列是一组分栏面板(`main-split`):面板宽 = 图标栏 + 侧边栏宽度,
//! [`Sidebar::pin_width`] 每帧把整列钉回期望宽度。「设置」入口在图标栏底部。

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Pixels,
    Render, Styled as _, Subscription, Window, px,
};
use gpui_kit::component::{
    Icon,
    empty::{Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyMediaVariant, EmptyTitle},
    resizable::ResizableState,
};

use crate::assets::IconName;
use crate::components::{ACTIVITY_BAR_WIDTH, SidebarRow};

mod activity_bar;
pub(crate) mod files;
mod header;
pub(crate) mod sessions;
mod shared;

use files::FilesState;
use sessions::SessionsState;

/// 面板标题栏的高度(标题那一行)。
pub(super) const PANEL_HEADER_HEIGHT: Pixels = px(28.);
/// 侧边栏默认宽度(分栏面板首次布局时的初始宽度)。
pub(crate) const SIDEBAR_DEFAULT_WIDTH: Pixels = px(220.);
/// 侧边栏窄到这个宽度(≈贴着图标栏)就折叠,见 [`Sidebar::set_panel_width`]。
const SIDEBAR_COLLAPSE_WIDTH: Pixels = px(24.);
/// 侧边栏那一组分栏面板里的下标(`main-split` 的第 0 个)。
const SIDEBAR_PANEL_INDEX: usize = 0;

/// 把拖拽后的**整列**宽度折算成侧边栏宽度;窄到贴着图标栏(≤ [`SIDEBAR_COLLAPSE_WIDTH`])
/// 返回 `None`,由调用方收起侧边栏。
fn sidebar_width_after_drag(panel_width: Pixels) -> Option<Pixels> {
    let width = panel_width - ACTIVITY_BAR_WIDTH;
    (width > SIDEBAR_COLLAPSE_WIDTH).then_some(width)
}

/// 侧边栏内容为空时的占位(gpui-kit [`Empty`]:虚线框 + 图标 + 标题 + 说明)。
///
/// 「会话」列表 0 条([`sessions`])与文件树没有可浏览的目录 / 目录是空的([`files`])共用它。
/// 覆盖组件默认值:内容位按内容定高(`.flex_none()`)、内边距收窄(`.p_3()`)。
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
    // 说明可选(一句话就够的地方不用凑第二行)。
    let header = match description {
        Some(text) => header.description(EmptyDescription::new().child(text)),
        None => header,
    };

    Empty::new().flex_none().p_3().header(header)
}

/// 侧边栏的视图（活动栏每枚图标一个、面板标题栏「…」菜单里一项）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SidebarView {
    /// 终端会话列表（记录树）。
    Sessions,
    /// 远端会话的文件树（本地会话下不摆，见 [`SidebarView::visible_with`]）。
    Files,
}

impl SidebarView {
    /// 全部视图(图标栏的图标与「…」菜单都按这个顺序摆)。
    pub(super) const ALL: [SidebarView; 2] = [Self::Sessions, Self::Files];

    /// 这个视图在当前会话下是否摆出来(「文件管理器」只对远端会话摆)。
    fn visible_with(self, files_enabled: bool) -> bool {
        match self {
            Self::Sessions => true,
            Self::Files => files_enabled,
        }
    }

    /// 图标栏那枚图标的图标名。
    fn icon(self) -> IconName {
        match self {
            Self::Sessions => IconName::SquareTerminal,
            Self::Files => IconName::Folder,
        }
    }

    /// 标签文字(面板标题栏的标题、「…」菜单项、图标栏的悬停提示)。
    fn label(self) -> &'static str {
        match self {
            Self::Sessions => "会话",
            Self::Files => "文件管理器",
        }
    }

    /// 无障碍名称(比标签多说一句)。
    fn tooltip(self) -> &'static str {
        match self {
            Self::Sessions => "终端会话",
            Self::Files => "当前终端目录下的内容",
        }
    }

    /// 元素 id / 面板 id 里用的短名。
    fn id(self) -> &'static str {
        match self {
            Self::Sessions => "sessions",
            Self::Files => "files",
        }
    }
}

// ---------------------------------------------------------------- 侧边栏实体

/// 最左侧整列(图标栏 + 侧边栏面板,全应用唯一一条)。
///
/// 状态与渲染都在这里:本实体实现 [`Render`],整列 = 48px 图标栏 + 侧边栏面板
/// (一直铺到窗口底部状态栏的上沿)。
pub(crate) struct Sidebar {
    /// 当前显示的视图(见 [`Sidebar::open_view`])。
    active: SidebarView,
    /// 「文件管理器」是否摆出来(根视图每帧同步,见 [`Sidebar::set_files_enabled`])。
    files_enabled: bool,
    /// 是否显示(折叠态只留那 48px 图标栏,见 [`Sidebar::toggle_view`])。
    visible: bool,
    /// 侧边栏面板的期望宽度(不含图标栏);由 [`Sidebar::pin_width`] 每帧钉回此值。
    width: Pixels,
    /// 这一组分栏面板的共享状态(`main-split`)。
    resize: Entity<ResizableState>,
    /// 上一次看到的容器宽度(供 [`Sidebar::pin_width`] 比对)。
    split_width: Option<Pixels>,
    /// 「会话」视图的状态(记录树)。
    sessions: Entity<SessionsState>,
    /// 「文件管理器」视图的状态(文件树)。
    files: Entity<FilesState>,
    /// 把两个视图的变化(增删改 / 展收 / 加载完成)转发成本实体的重绘。
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
            // 默认显示「会话」。
            active: SidebarView::Sessions,
            // 先按不可见起步:根视图在首次渲染前会按当前会话同步一次。
            files_enabled: false,
            // 默认折叠。
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

    /// 本栏是否可见。
    pub(crate) fn visible(&self) -> bool {
        self.visible
    }

    /// 设置是否显示(图标栏点当前图标即折叠 / 展开;值没变时什么都不做)。
    pub(crate) fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        self.on_visibility_change();
        cx.notify();
    }

    /// 同步「文件管理器」的可用性(根视图每帧按当前会话是不是远端调用):
    /// 不可用时该视图的图标与内容一起不摆([`SidebarView::visible_with`])。
    pub(crate) fn set_files_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.files_enabled == enabled {
            return;
        }
        self.files_enabled = enabled;
        cx.notify();
    }

    /// 侧边栏面板的期望宽度(不含图标栏 —— 整列的面板宽 = 二者之和)。
    pub(crate) fn width(&self) -> Pixels {
        self.width
    }

    /// 记下拖拽后的整列宽度(分栏的 `on_resize` 回调):减掉图标栏才是侧边栏宽度;
    /// 拖到贴着图标栏(≤ [`SIDEBAR_COLLAPSE_WIDTH`])则收起侧边栏,并保留收起前的宽度。
    pub(crate) fn set_panel_width(&mut self, panel_width: Pixels, cx: &mut Context<Self>) {
        let Some(width) = sidebar_width_after_drag(panel_width) else {
            self.set_visible(false, cx);
            return;
        };
        self.width = width;
    }

    /// 可见性变化后的收尾:清掉 `split_width`(下一次 [`Sidebar::pin_width`] 会重新钉一次宽度)。
    fn on_visibility_change(&mut self) {
        self.split_width = None;
    }

    /// 本栏那一组分栏面板的共享状态(根视图装配 `h_resizable` 时用)。
    pub(crate) fn resize_state(&self) -> &Entity<ResizableState> {
        &self.resize
    }

    /// 容器宽度变过就 `resize_panel` 把整列钉回「图标栏 + 期望宽度」
    /// (在 `AppRoot::render` 开头调用)。折叠态整列不在分栏组里,直接返回。
    pub(crate) fn pin_width(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        let width = ACTIVITY_BAR_WIDTH + self.width;
        let container = self.resize.read(cx).container_size();
        if self.split_width != Some(container) {
            self.split_width = Some(container);
            let index = SIDEBAR_PANEL_INDEX;
            self.resize.update(cx, |state, cx| {
                state.resize_panel(index, width, window, cx);
            });
        }
    }

    /// 让本栏可见(打开视图就意味着「用户在看它」)。
    fn show(&mut self) {
        self.visible = true;
        self.on_visibility_change();
    }

    /// 当前该摆出来的视图:选中的那个不可用(本地会话下的「文件管理器」)就退到第一个可用的
    /// 视图;一个可用的都没有则 `None`(内容位摆空占位)。
    fn active_view(&self) -> Option<SidebarView> {
        if self.active.visible_with(self.files_enabled) {
            return Some(self.active);
        }
        SidebarView::ALL
            .into_iter()
            .find(|view| view.visible_with(self.files_enabled))
    }

    /// 图标栏点选:让侧边栏显示这个视图;再点**正在显示**的那一枚 = 折叠侧边栏。
    pub(crate) fn toggle_view(&mut self, view: SidebarView, cx: &mut Context<Self>) {
        if self.visible && self.active_view() == Some(view) {
            self.set_visible(false, cx);
            return;
        }
        self.open_view(view, cx);
    }

    /// 打开某个视图(图标栏点选 / 「…」菜单):切到它并让侧边栏可见。
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
        // 整列的元素由 `components/` 里的组件画,这里只把状态交给装配入口。
        activity_bar::left_column(self, cx)
    }
}

/// 侧边栏内容:两份视图混合的一组行,用一个枚举统一类型交给
/// [`crate::components::Sidebar`](它只认一种行类型)。
#[derive(Clone)]
enum SidebarContent {
    /// 「会话」列表的一项。
    Sessions(sessions::SessionsItem),
    /// 「文件管理器」的一项(路径输入框 / 一行 / 空占位)。
    Files(files::FilesItem),
}

impl SidebarRow for SidebarContent {
    /// 按需把这一行画出来(行数据自带渲染所需的句柄)。
    fn render(self, cx: &mut App) -> AnyElement {
        match self {
            Self::Sessions(item) => item.render(cx),
            Self::Files(item) => item.render(cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ACTIVITY_BAR_WIDTH, SIDEBAR_COLLAPSE_WIDTH, sidebar_width_after_drag};
    use gpui::px;

    #[test]
    fn sidebar_width_has_no_min_or_max() {
        assert_eq!(
            sidebar_width_after_drag(ACTIVITY_BAR_WIDTH + px(25.)),
            Some(px(25.))
        );
        assert_eq!(
            sidebar_width_after_drag(ACTIVITY_BAR_WIDTH + px(2000.)),
            Some(px(2000.))
        );
    }

    #[test]
    fn dragging_flush_against_the_icon_bar_collapses() {
        // 只剩图标栏宽(及更窄)⇒ 收起。
        assert_eq!(sidebar_width_after_drag(ACTIVITY_BAR_WIDTH), None);
        assert_eq!(sidebar_width_after_drag(ACTIVITY_BAR_WIDTH - px(10.)), None);
        // 阈值以内也算收起。
        assert_eq!(
            sidebar_width_after_drag(ACTIVITY_BAR_WIDTH + SIDEBAR_COLLAPSE_WIDTH),
            None
        );
    }
}
