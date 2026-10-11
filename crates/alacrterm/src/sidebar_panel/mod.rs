//! 侧边栏(全应用唯一一条,在活动栏右侧):顶部面板标题栏 + 视图内容,一直铺到窗口底部
//! 那条状态栏的上沿(本栏没有状态栏)。
//!
//! **侧边栏 = 一个实体**([`Sidebar`]):自带当前视图、折叠状态、期望宽度、那一组分栏面板
//! 状态与自己的渲染;根视图只把它 `.child(..)` 摆进分栏面板、把 [`activity_bar`] 摆好。
//!
//! 视图内容是两个共享实体:[`sessions::SessionsState`](「会话」)与
//! [`files::FilesState`](「文件管理器」)——同一时刻只摆一个(另一个仍活着、状态不丢)。
//! 后者**只对远端会话可见**(本地目录用系统自己的文件管理器即可),可用性由根视图每帧
//! 按当前会话同步进来([`Sidebar::set_files_enabled`])。
//!
//! 其余分工(**当前是哪个视图不再靠一排标签**,照 VS Code 的侧边栏):
//! - **活动栏**([`activity_bar`]):最左一列的图标(每个视图一枚 + 底部「设置」),常驻不折叠;
//!   点一枚 = 让侧边栏显示这个视图,再点当前那枚 = 折叠侧边栏。
//! - **面板标题栏**([`header`]):挂在 `Sidebar::header` 上(固定顶部、不随内容滚动),摆当前
//!   视图的标题 + 右端「…」菜单(列出各视图、勾选当前那个)。
//! - **「会话」视图的右键菜单**(右键落在空白处 / 面板标题上):顶层「新建会话」([`NewSession`])、
//!   「新建文件夹」([`NewFolder`])—— 原来是那条状态栏两端的两枚按钮;两个都只加列表条目、
//!   不开终端(见 [`crate::dialog`])。⚠️ 条目自己的右键菜单挂在条目身上(见 [`sessions`])。
//! - **空内容占位**([`empty_state`]):视图自己没内容(会话列表 0 条、目录读不出 / 是空的)时
//!   统一摆 gpui-kit 的 `Empty`。⚠️ **一个可用视图都没有**时(只剩本地会话下不摆的
//!   「文件管理器」)什么都不摆 —— 只留侧边栏背景。
//!
//! 侧边栏与中间列是**一组**分栏面板(`main-split`):面板宽度按下标存在 `ResizableState` 里,
//! [`Sidebar::pin_width`] 每帧把侧边栏宽度钉回期望值。
//!
//! 「设置」入口在**活动栏底部**(见 [`activity_bar`])。

use gpui::{
    App, AppContext as _, Context, ElementId, Entity, IntoElement, ParentElement as _, Pixels,
    Render, SharedString, Styled as _, Subscription, Window, div, px,
};
use gpui_kit::component::{
    Collapsible, Icon,
    empty::{Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyMediaVariant, EmptyTitle},
    menu::ContextMenuExt as _,
    resizable::ResizableState,
    sidebar::{Sidebar as SidebarWidget, SidebarItem},
    v_flex,
};

use crate::actions::{NewFolder, NewSession};
use crate::assets::IconName;

pub(crate) mod files;
pub(crate) mod sessions;
mod activity_bar;
mod header;
mod shared;

pub(crate) use activity_bar::activity_bar;

use files::FilesState;
use sessions::SessionsState;

/// 面板标题栏的高度(标题那一行;VS Code 的侧边栏标题栏约 35px,这里与树行 / 状态栏取齐)。
pub(super) const PANEL_HEADER_HEIGHT: Pixels = px(28.);
/// 侧边栏默认宽度（分栏面板首次布局时的初始宽度）。
pub(crate) const SIDEBAR_DEFAULT_WIDTH: Pixels = px(220.);
/// 侧边栏拖拽时的最小宽度（须大于组件内部的 `PANEL_MIN_SIZE` = 100px）。
pub(crate) const SIDEBAR_MIN_WIDTH: Pixels = px(150.);
/// 侧边栏拖拽时的最大宽度。
pub(crate) const SIDEBAR_MAX_WIDTH: Pixels = px(460.);
/// 侧边栏在它那一组分栏面板里的下标（`main-split` 的第 0 个 = 侧边栏，第 1 个 = 中间列）。
const SIDEBAR_PANEL_INDEX: usize = 0;

/// 侧边栏内容为空时的占位（gpui-kit [`Empty`]：虚线框 + 图标 + 标题 + 说明）。
///
/// 两处空内容共用它：「会话」列表一条都没有（[`sessions`]）、文件树没有可浏览的目录 /
/// 目录是空的（[`files`]）。
///
/// ⚠️ 覆盖两处组件默认值：`.flex_none()`（`Empty` 自带 `flex_1`，而这里的内容位是
/// **按内容定高**的 ⇒ 不钉住会被压成 0 高）、`.p_3()`（默认 `p_6`，侧边栏最窄只有 150px）。
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
    // 说明是可选的：一句话就够的地方（比如空目录）不用凑第二行。
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
    /// 全部视图(活动栏的图标与「…」菜单都按这个顺序摆)。
    pub(super) const ALL: [SidebarView; 2] = [Self::Sessions, Self::Files];

    /// 这个视图在当前会话下是否该摆出来。
    ///
    /// 「文件管理器」只服务**远端（SSH）会话**：本地目录用系统自己的文件管理器打开就好，
    /// 应用里再摆一份既多余、又只能看到本机目录；所以本地会话（以及没有会话）时这个视图
    /// 连同它的图标一起不摆。可用性由根视图每帧同步（`files_enabled`）。
    fn visible_with(self, files_enabled: bool) -> bool {
        match self {
            Self::Sessions => true,
            Self::Files => files_enabled,
        }
    }

    /// 视图的图标(活动栏那枚按钮)。
    fn icon(self) -> IconName {
        match self {
            Self::Sessions => IconName::SquareTerminal,
            Self::Files => IconName::Folder,
        }
    }

    /// 标签文字(面板标题栏的标题、「…」菜单项)。
    fn label(self) -> &'static str {
        match self {
            Self::Sessions => "会话",
            Self::Files => "文件管理器",
        }
    }

    /// 标签提示（文字太短，鼠标停留时补一句说明）。
    fn tooltip(self) -> &'static str {
        match self {
            Self::Sessions => "终端会话",
            Self::Files => "当前终端目录下的内容",
        }
    }

    /// 元素 id / `Sidebar` id 里用的短名。
    fn id(self) -> &'static str {
        match self {
            Self::Sessions => "sessions",
            Self::Files => "files",
        }
    }
}

// ---------------------------------------------------------------- 侧边栏实体

/// 侧边栏（全应用唯一一个）。
///
/// **状态与渲染都在这里**：本实体实现 [`Render`]，整列 = 面板标题栏 + 当前视图内容
/// （一直铺到窗口底部状态栏的上沿）；根视图只负责把它摆进分栏面板、把活动栏摆好。
pub(crate) struct Sidebar {
    /// 当前显示的视图（活动栏点图标即切换，见 [`Sidebar::open_view`]）。
    active: SidebarView,
    /// 「文件管理器」是否摆出来（只对远端会话可见，根视图每帧同步，见
    /// [`Sidebar::set_files_enabled`]）。不影响当前选中的视图：会话换回远端时它自己回来。
    files_enabled: bool,
    /// 是否显示（折叠态整列不渲染，只留最左的活动栏，见 [`Sidebar::toggle_view`]）。
    visible: bool,
    /// 侧边栏的「期望宽度」（逻辑像素）：用户拖拽分隔条后的宽度记在这里。
    ///
    /// 分栏容器在容器尺寸变化时会把面板按比例重排，于是窗口一变宽、侧边栏就跟着变宽；
    /// 记下期望宽度后由 [`Sidebar::pin_width`] 把面板钉回去，让宽窄变化全部由中间那一列吸收。
    width: Pixels,
    /// 这一组分栏面板的共享状态（`main-split`）。
    resize: Entity<ResizableState>,
    /// 上一次看到的容器宽度：分栏容器只在**容器宽度变化**的那次布局里重排面板，
    /// 所以「和上次不一样」就等于「刚发生过重排」——[`Sidebar::pin_width`] 据此决定动不动手。
    split_width: Option<Pixels>,
    /// 「会话」视图的状态（记录树）。
    sessions: Entity<SessionsState>,
    /// 「文件管理器」视图的状态（文件树）。
    files: Entity<FilesState>,
    /// 把「会话」/「文件管理器」的变化（增删改 / 展收 / 加载完成）转发一次重绘。
    ///
    /// ⚠️ 必须转发：两个视图都不再自己实现 `Render`（行由本实体的内容区渲染），
    /// 它们的 `notify` 落不到窗口上。
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
            // 默认显示「会话」；「文件管理器」也在这一栏里，靠活动栏的图标切过去。
            active: SidebarView::Sessions,
            // 先按「不可见」起步：根视图在首次渲染前就会按当前会话同步一次。
            files_enabled: false,
            // 默认折叠：启动只留标题栏 + 活动栏 + 中间列，展开靠活动栏的图标。
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

    /// 设置是否显示（活动栏点当前图标即折叠 / 展开；值没变时什么都不做）。
    pub(crate) fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        cx.notify();
    }

    /// 同步「文件管理器」的可用性（根视图每帧按当前会话是不是远端调用）。
    ///
    /// 不可用时该视图的图标与内容一起不摆（[`SidebarView::visible_with`]）；当前选中的视图
    /// 本身不动，所以会话换回远端后它还会自己回来。
    pub(crate) fn set_files_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.files_enabled == enabled {
            return;
        }
        self.files_enabled = enabled;
        cx.notify();
    }

    /// 本栏的「期望宽度」。
    pub(crate) fn width(&self) -> Pixels {
        self.width
    }

    /// 记下拖拽后的宽度（分栏的 `on_resize` 回调里调）。
    pub(crate) fn set_width(&mut self, width: Pixels) {
        self.width = width;
    }

    /// 本栏那一组分栏面板的共享状态（根视图装配 `h_resizable` 时用）。
    pub(crate) fn resize_state(&self) -> &Entity<ResizableState> {
        &self.resize
    }

    /// 确保本栏保持「期望宽度」，宽度变化全部由中间那一列吸收（在 `AppRoot::render` 开头调）。
    ///
    /// 分栏容器在容器尺寸变化时会把**所有**面板按比例重排
    /// （`ResizableState::adjust_to_container_size`）——于是缩放窗口、折叠另一侧边栏
    /// 都会把侧边栏一起带宽 / 带窄。这里发现容器宽度与上次不同（即刚重排过）就立刻
    /// `resize_panel` 钉回 [`Sidebar::width`]；多出来 / 少掉的空间自然落到中间那一列。
    ///
    /// 「只在容器宽度变过时才动手」很关键：拖拽分隔条不改变容器宽度，所以不会和用户抢宽度
    /// （拖拽结果由 `on_resize` 回调记进 [`Sidebar::set_width`]）。期望宽度与当前宽度相同时
    /// `ResizableState::resize_panel` 直接返回，因此可以每帧调用。
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

    /// 让本栏可见（打开视图就意味着「用户在看它」）。
    fn show(&mut self) {
        self.visible = true;
    }

    /// 当前该摆出来的视图。
    ///
    /// 选中的那个不可用（本地会话下的「文件管理器」）就退到第一个可用的视图；一个可用的
    /// 都没有则 `None`（内容位摆空占位）。**选中的那个不动**——会话换回远端时它自己回来。
    fn active_view(&self) -> Option<SidebarView> {
        if self.active.visible_with(self.files_enabled) {
            return Some(self.active);
        }
        SidebarView::ALL
            .into_iter()
            .find(|view| view.visible_with(self.files_enabled))
    }

    /// 活动栏点选：让侧边栏显示这个视图；再点**正在显示**的那一枚 = 折叠侧边栏
    /// —— 照 VS Code，折叠不另设开关。
    pub(crate) fn toggle_view(&mut self, view: SidebarView, cx: &mut Context<Self>) {
        if self.visible && self.active_view() == Some(view) {
            self.set_visible(false, cx);
            return;
        }
        self.open_view(view, cx);
    }

    /// 打开某个视图（活动栏点选 / 「…」菜单）：切到它并让侧边栏可见。
    pub(crate) fn open_view(&mut self, view: SidebarView, cx: &mut Context<Self>) {
        let was_visible = self.visible;
        let changed = self.active != view;
        self.active = view;
        // 视图只在侧边栏可见时渲染，所以这里其实必然是 true；
        // 保留赋值是为了让「打开视图 ⇒ 侧边栏可见」这个不变量不依赖渲染条件。
        self.show();
        if changed || !was_visible {
            cx.notify();
        }
    }
}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_view();
        // 内容 = **一组 item**（不一定是 1 项）：文件树一行一项，交给组件内部的虚拟列表，
        // 由它按可见区间渲染（见 [`SidebarContent`] 与 `files` 模块文档）。
        let contents: Vec<SidebarContent> = match active {
            Some(SidebarView::Sessions) => self
                .sessions
                .update(cx, |sessions, cx| sessions.sidebar_items(cx))
                .into_iter()
                .map(SidebarContent::Sessions)
                .collect(),
            Some(SidebarView::Files) => self
                .files
                .update(cx, |files, cx| files.sidebar_items(cx))
                .into_iter()
                .map(SidebarContent::Files)
                .collect(),
            // 没有可见的标签（全被拖走，或只剩本地会话下不摆的「文件管理器」）：
            // 内容位**什么都不摆**，只留侧边栏背景。
            None => Vec::new(),
        };

        // 宽度由外层分栏面板决定：必须 w_full，否则会回落到组件内置默认宽度。
        // flex_1 + min_h_0：纵向要能被压（本栏一直铺到窗口底部状态栏的上沿，下面没有自己的状态栏）。
        // 宽度由外层分栏面板决定：必须 w_full，否则会回落到组件内置默认宽度。
        // flex_1 + min_h_0：纵向要能被压（本栏一直铺到窗口底部状态栏的上沿，下面没有自己的状态栏）。
        // ⚠️ 组件默认就是左栏（`Side::Left`，边框画在右边）—— 应用只有一个侧边栏，无需再指定。
        let widget = SidebarWidget::new(SharedString::from(format!(
            "sidebar-{}",
            active.map_or("empty", SidebarView::id)
        )))
        .w_full()
        // 顶部固定的面板标题栏（不随下方内容滚动）。
        .header(self.render_header(cx))
        // 一组内容项：会话视图 1 项；文件管理器一行一项（组件内部虚拟列表只渲染可见行）。
        .children(contents)
        .flex_1()
        .min_h_0();

        // 「会话」视图：整块内容的右键菜单 = 顶层「新建会话 / 新建文件夹」——
        // 原来是本栏状态栏两端那两枚按钮（状态栏去掉后搬到这里）。⚠️ 条目自己的右键菜单挂在
        // 条目身上（见 [`sessions`]），所以右键落在空白处 / 面板标题上才是这一套。
        let body = match active {
            Some(SidebarView::Sessions) => div()
                .flex_1()
                .min_h_0()
                .context_menu(move |menu, _window, _cx| {
                    menu.menu_with_icon(
                        "新建会话",
                        IconName::Plus,
                        Box::new(NewSession { folder: None }),
                    )
                    .menu_with_icon(
                        "新建文件夹",
                        IconName::FolderPlus,
                        Box::new(NewFolder { parent: None }),
                    )
                })
                .child(widget)
                .into_any_element(),
            _ => widget.into_any_element(),
        };

        // 侧边栏一直铺到**窗口底部那条状态栏的上沿**：这一栏没有自己的状态栏
        // （原来那两枚新建按钮已进上面的右键菜单）。
        v_flex()
            .h_full()
            .w_full()
            .overflow_hidden()
            .child(body)
    }
}

/// `SidebarWidget::children` 只接受单一类型，而侧边栏内容是「两份视图混合的一组项」
/// ⇒ 用一个枚举把类型统一起来。两个视图都是**一行一项**（见 [`files::FilesItem`]）。
#[derive(Clone)]
enum SidebarContent {
    /// 「会话」列表的一项。
    Sessions(sessions::SessionsItem),
    /// 「文件管理器」的一项（路径输入框 / 一行 / 空占位）。
    Files(files::FilesItem),
}

impl Collapsible for SidebarContent {
    fn is_collapsed(&self) -> bool {
        false
    }

    fn collapsed(self, _: bool) -> Self {
        self
    }
}

impl SidebarItem for SidebarContent {
    /// 两个分支都不用 `id` / `window`（渲染与交互都在行元素自己身上），故加下划线。
    fn render(
        self,
        _id: impl Into<ElementId>,
        _window: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement {
        match self {
            // 两个视图都是读行数据 / 接交互在 `render` 里自己干的。
            Self::Sessions(item) => item.render(cx),
            Self::Files(item) => item.render(cx),
        }
    }
}
