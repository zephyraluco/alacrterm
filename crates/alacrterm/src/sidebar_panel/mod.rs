//! 侧边栏(左侧 / 右侧):顶部可拖动的视图标签 + 视图内容 + 底部状态栏。
//!
//! **一条侧边栏 = 一个实体**([`Sidebar`],左右各一个):自带标签、折叠状态、期望宽度、
//! 那一组分栏面板状态与自己的渲染;根视图只把它 `.child(..)` 摆进分栏面板。
//!
//! 视图内容是两个共享实体:[`sessions::SessionsState`](「会话」)与
//! [`files::FilesState`](「文件管理器」)——标签落在哪一侧,就由哪一侧把它摆出来。
//! 后者**只对远端会话可见**(本地目录用系统自己的文件管理器即可),可用性由根视图每帧
//! 按当前会话同步进来([`Sidebar::set_files_enabled`])。
//!
//! 其余分工:
//! - **视图标签条**([`tabs`]):挂在 `Sidebar::header` 上(固定顶部、不随内容滚动);
//!   标签可在两条侧边栏之间拖动:同栏内换位是本实体自己的事,跨栏经 `sibling` 弱引用
//!   从另一条实体取视图。
//! - **折叠开关**([`toggle_button`]):渲染在**标题栏右端**(由根视图摆放),折叠后仍点得到。
//! - **两枚状态栏按钮**(显示「会话」视图的那条才有):左下 = 新建文件夹([`NewFolder`])、
//!   右下 = 新建会话([`NewSession`]),都只加列表条目、不开终端(见 [`crate::dialog`])。
//! - **空内容占位**([`empty_state`]):视图自己没内容(会话列表 0 条、目录读不出 / 是空的)时
//!   统一摆 gpui-kit 的 `Empty`。⚠️ 内容位**一个可见标签都没有**时(标签全被拖走,或只剩
//!   本地会话下不摆的「文件管理器」)什么都不摆 —— 只留侧边栏背景。
//!
//! 两侧边栏用**两组嵌套的分栏面板**装配(内层 `main-split`、外层 `right-split`):面板宽度
//! 按下标存在 `ResizableState` 里,三块面板挤在同一组会互相影响下标。[`Sidebar::pin_width`]
//! 每帧把宽度钉回期望值。
//!
//! 「设置」入口在标题栏左侧的文字按钮上(见 [`crate::AppRoot::render`]),不在本模块。

use gpui::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Render, SharedString, Styled as _, WeakEntity, Window,
    div, px,
};
use gpui_kit::component::{
    Collapsible, Icon, Side, Sizable as _,
    button::{Button, ButtonVariants as _},
    empty::{Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyMediaVariant, EmptyTitle},
    resizable::ResizableState,
    sidebar::{Sidebar as SidebarWidget, SidebarItem},
    status_bar::StatusBar,
    v_flex,
};

use crate::AppRoot;
use crate::actions::{NewFolder, NewSession};
use crate::assets::IconName;
use crate::status_bar::STATUS_BAR_HEIGHT;

pub(crate) mod files;
pub(crate) mod sessions;
mod tabs;

use files::FilesState;
use sessions::SessionsState;

/// 顶部视图标签条的高度(标签本身与拖拽预览卡片共用)。
pub(super) const TAB_HEIGHT: Pixels = px(24.);
/// 左侧边栏默认宽度（分栏面板首次布局时的初始宽度）。
pub(crate) const SIDEBAR_DEFAULT_WIDTH: Pixels = px(220.);
/// 侧边栏拖拽时的最小宽度（须大于组件内部的 `PANEL_MIN_SIZE` = 100px）。
pub(crate) const SIDEBAR_MIN_WIDTH: Pixels = px(150.);
/// 侧边栏拖拽时的最大宽度。
pub(crate) const SIDEBAR_MAX_WIDTH: Pixels = px(460.);

/// 右侧边栏默认宽度（分栏面板首次布局时的初始宽度）。
pub(crate) const RIGHT_SIDEBAR_DEFAULT_WIDTH: Pixels = px(240.);
/// 右侧边栏拖拽时的最小宽度（须大于组件内部的 `PANEL_MIN_SIZE` = 100px）。
pub(crate) const RIGHT_SIDEBAR_MIN_WIDTH: Pixels = px(150.);
/// 右侧边栏拖拽时的最大宽度。
pub(crate) const RIGHT_SIDEBAR_MAX_WIDTH: Pixels = px(460.);

/// 侧边栏的哪一侧（标签可以在这两侧之间拖动）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SidebarSide {
    Left,
    Right,
}

impl SidebarSide {
    /// 元素 id 里用的短名。
    fn id(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
        }
    }

    /// 该侧边栏在**自己那一组**分栏面板里的下标。
    ///
    /// 左栏是 `main-split` 的第 0 个面板（后面还有中间列）；右栏是 `right-split` 的
    /// 第 1 个面板（最后一个，`resize_panel` 会挤压前一个面板来给它让位）。
    fn panel_index(self) -> usize {
        match self {
            Self::Left => 0,
            Self::Right => 1,
        }
    }
}

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

/// 可以停放在任一侧边栏的视图（[`SidebarTabs`] 里的元素）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SidebarView {
    /// 终端会话列表（记录树）。
    Sessions,
    /// 远端会话的文件树（本地会话下不摆，见 [`SidebarView::visible_with`]）。
    Files,
}

impl SidebarView {
    /// 这个视图在当前会话下是否该摆出来。
    ///
    /// 「文件管理器」只服务**远端（SSH）会话**：本地目录用系统自己的文件管理器打开就好，
    /// 应用里再摆一份既多余、又只能看到本机目录；所以本地会话（以及没有会话）时这个视图
    /// 连同它的标签一起不摆。可用性由根视图每帧同步（`files_enabled`）。
    fn visible_with(self, files_enabled: bool) -> bool {
        match self {
            Self::Sessions => true,
            Self::Files => files_enabled,
        }
    }

    /// 标签文字。
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

/// 一条侧边栏上的标签：**顺序 + 当前选中**（标签可以拖到另一条侧边栏，见 [`tabs`]）。
#[derive(Clone)]
pub(crate) struct SidebarTabs {
    views: Vec<SidebarView>,
    /// 选中项下标；标签条为空时无意义。
    active: usize,
}

impl SidebarTabs {
    pub(crate) fn new(views: Vec<SidebarView>) -> Self {
        Self { views, active: 0 }
    }

    /// 当前选中的视图（标签条为空时 `None`）。
    fn active_view(&self) -> Option<SidebarView> {
        self.views.get(self.active).copied()
    }

    /// 点击第 `ix` 个标签：选中它。返回是否发生了改变（没改变就不用重绘）。
    fn select(&mut self, ix: usize) -> bool {
        if ix >= self.views.len() || ix == self.active {
            return false;
        }
        self.active = ix;
        true
    }

    /// 取出第 `ix` 个标签的视图（拖走时用），选中项跟着挪到仍存在的标签上。
    fn take(&mut self, ix: usize) -> Option<SidebarView> {
        if ix >= self.views.len() {
            return None;
        }
        let view = self.views.remove(ix);
        // 抽走的是选中项之前的 ⇒ 下标前移；抽走的就是选中项 ⇒ 顺延到后一个
        // （已是末尾则回到前一个，空列表归 0）。
        if ix < self.active {
            self.active -= 1;
        }
        self.active = self.active.min(self.views.len().saturating_sub(1));
        Some(view)
    }

    /// 在第 `ix` 个位置插入并**选中**它（`ix` 越界则追加到末尾）。
    fn insert_active(&mut self, ix: usize, view: SidebarView) {
        let ix = ix.min(self.views.len());
        self.views.insert(ix, view);
        self.active = ix;
    }
}

// ---------------------------------------------------------------- 侧边栏实体

/// 一条侧边栏（左 / 右各一个实体）。
///
/// **状态与渲染都在这里**：本实体实现 [`Render`]，整列 = 视图标签条 + 当前视图内容 +
/// 底部状态栏；根视图只负责把它摆进分栏面板、把 [`toggle_button`] 摆进标题栏。
pub(crate) struct Sidebar {
    /// 我是哪一条（决定 `Side::Right`、分栏面板下标、元素 id）。
    side: SidebarSide,
    tabs: SidebarTabs,
    /// 「文件管理器」是否摆出来（只对远端会话可见，根视图每帧同步，见
    /// [`Sidebar::set_files_enabled`]）。不影响标签数据本身：会话换回远端时它自己回来。
    files_enabled: bool,
    visible: bool,
    /// 本栏的「期望宽度」（逻辑像素）：用户拖拽分隔条后的宽度记在这里。
    ///
    /// 分栏容器在容器尺寸变化时会把面板按比例重排，于是窗口一变宽、侧边栏就跟着变宽；
    /// 记下期望宽度后由 [`Sidebar::pin_width`] 把面板钉回去，让宽窄变化全部由中间那一列吸收。
    width: Pixels,
    /// 本栏那一组分栏面板的共享状态（左栏 = `main-split`，右栏 = `right-split`）。
    resize: Entity<ResizableState>,
    /// 上一次看到的容器宽度：分栏容器只在**容器宽度变化**的那次布局里重排面板，
    /// 所以「和上次不一样」就等于「刚发生过重排」——[`Sidebar::pin_width`] 据此决定动不动手。
    split_width: Option<Pixels>,
    /// 「会话」视图的状态（记录树）：两条侧边栏共用同一个实体。
    sessions: Entity<SessionsState>,
    /// 「文件管理器」视图的状态（文件树）：两条侧边栏共用同一个实体。
    files: Entity<FilesState>,
    /// 另一条侧边栏：标签跨栏拖动时要把视图从它那儿取过来。
    sibling: Option<WeakEntity<Sidebar>>,
}

impl Sidebar {
    pub(crate) fn new(
        side: SidebarSide,
        sessions: Entity<SessionsState>,
        files: Entity<FilesState>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            side,
            // 默认：左栏是「会话」，右栏是「文件管理器」（都可以拖动改变）。
            tabs: match side {
                SidebarSide::Left => SidebarTabs::new(vec![SidebarView::Sessions]),
                SidebarSide::Right => SidebarTabs::new(vec![SidebarView::Files]),
            },
            // 先按「不可见」起步：根视图在首次渲染前就会按当前会话同步一次。
            files_enabled: false,
            // 默认折叠：启动只留标题栏 + 中间列 + 状态栏，展开靠标题栏右端那两枚开关。
            visible: false,
            width: match side {
                SidebarSide::Left => SIDEBAR_DEFAULT_WIDTH,
                SidebarSide::Right => RIGHT_SIDEBAR_DEFAULT_WIDTH,
            },
            resize: cx.new(|_| ResizableState::default()),
            split_width: None,
            sessions,
            files,
            sibling: None,
        }
    }

    /// 接上另一条侧边栏（两条都建好后由根视图调用一次）。
    pub(crate) fn connect(&mut self, sibling: WeakEntity<Sidebar>) {
        self.sibling = Some(sibling);
    }

    fn other(&self) -> Option<WeakEntity<Sidebar>> {
        self.sibling.clone()
    }

    /// 本栏是否可见。
    pub(crate) fn visible(&self) -> bool {
        self.visible
    }

    /// 设置是否显示（只由标题栏里的折叠开关调用；值没变时什么都不做）。
    pub(crate) fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        cx.notify();
    }

    /// 同步「文件管理器」的可用性（根视图每帧按当前会话是不是远端调用）。
    ///
    /// 不可用时该视图的标签与内容一起不摆（[`SidebarView::visible_with`]）；标签数据
    /// 本身不动，所以会话换回远端后它还会在原来的位置上。
    pub(crate) fn set_files_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.files_enabled == enabled {
            return;
        }
        self.files_enabled = enabled;
        cx.notify();
    }

    /// 折叠 / 展开（标题栏那枚开关）。
    pub(crate) fn toggle(&mut self, cx: &mut Context<Self>) {
        let visible = self.visible;
        self.set_visible(!visible, cx);
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
            let index = self.side.panel_index();
            self.resize.update(cx, |state, cx| {
                state.resize_panel(index, width, window, cx);
            });
        }
    }

    /// 让本栏可见（拖放落地时意味着「用户在看它」）。
    fn show(&mut self) {
        self.visible = true;
    }

    /// 当前该摆出来的视图。
    ///
    /// 选中的那个不可用（本地会话下的「文件管理器」）就退到第一个可用的标签；一个可用的
    /// 都没有则 `None`（内容位摆空占位）。**选中的下标不动**——会话换回远端时它自己回来。
    fn active_view(&self) -> Option<SidebarView> {
        match self.tabs.active_view() {
            Some(view) if view.visible_with(self.files_enabled) => Some(view),
            _ => self
                .tabs
                .views
                .iter()
                .copied()
                .find(|view| view.visible_with(self.files_enabled)),
        }
    }

    /// 点击标签：切到它代表的视图。
    ///
    /// **只切视图，不改可见性**——折叠 / 展开只由标题栏那枚开关负责。
    /// 因此点击**当前**标签是空操作。
    fn select_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if !self.tabs.select(index) {
            return;
        }
        // 标签只在侧边栏可见时渲染，所以这里其实必然是 true；
        // 保留赋值是为了让「切视图 ⇒ 侧边栏可见」这个不变量不依赖渲染条件。
        self.show();
        cx.notify();
    }

    /// 把拖动的标签放到本栏的第 `index` 个标签处（占它的位置）。
    ///
    /// `index` 是**拖动前**本栏里的下标：先插入、再选中它，所以「拖到第 i 个标签上」的结果
    /// 就是「这个视图落在第 i 个位置、其余顺延」。同一栏内拖动也是同一套语义（先抽走再按
    /// 原下标插入），因此「把 A 拖到 B 上」在 `[A, B]` 这种只有两项时也能得到 `[B, A]`。
    ///
    /// 来源是**另一条**侧边栏时，经 `sibling` 把视图从那条实体取出来（它自己会重绘）。
    fn drop_tab(&mut self, drag: &tabs::DragSidebarTab, index: usize, cx: &mut Context<Self>) {
        let Some(view) = self.take_dragged(drag, cx) else {
            return;
        };
        self.tabs.insert_active(index, view);
        self.show();
        cx.notify();
    }

    /// 追加到本栏末尾（拖到标签条空白处，或本栏为空时的兑现落点）。
    fn append_tab(&mut self, drag: &tabs::DragSidebarTab, cx: &mut Context<Self>) {
        let Some(view) = self.take_dragged(drag, cx) else {
            return;
        };
        self.tabs.insert_active(usize::MAX, view);
        self.show();
        cx.notify();
    }

    /// 从拖动来源取出那个视图：来源是本栏就本地取，是另一条侧边栏就更新那条实体。
    fn take_dragged(
        &mut self,
        drag: &tabs::DragSidebarTab,
        cx: &mut Context<Self>,
    ) -> Option<SidebarView> {
        if drag.side == self.side {
            return self.tabs.take(drag.index);
        }
        let sibling = self.other()?;
        let index = drag.index;
        sibling
            .update(cx, |sibling, cx| {
                let view = sibling.tabs.take(index);
                if view.is_some() {
                    cx.notify();
                }
                view
            })
            .ok()
            .flatten()
    }
}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_view();
        // `Sidebar::child` 只吃单一类型，而两类内容（会话树实体 / 内置菜单）类型不同
        // ⇒ 用 [`SidebarContent`] 统一。
        let content = match active {
            Some(SidebarView::Sessions) => SidebarContent::Sessions(self.sessions.clone()),
            Some(SidebarView::Files) => SidebarContent::Files(self.files.clone()),
            // 没有可见的标签（全被拖走，或只剩本地会话下不摆的「文件管理器」）：
            // 内容位**什么都不摆**，只留侧边栏背景。
            None => SidebarContent::Empty,
        };

        // 宽度由外层分栏面板决定：必须 w_full，否则会回落到组件内置默认宽度。
        // flex_1 + min_h_0：与下方**本栏自己的状态栏**同处一列，需能收缩。
        // 空白区域**不挂**右键菜单，只有会话条目有自己的右键菜单。
        let widget = SidebarWidget::new(SharedString::from(format!(
            "sidebar-{}-{}",
            self.side.id(),
            active.map_or("empty", SidebarView::id)
        )))
        .w_full()
        // 顶部固定的标签条（不随下方内容滚动）。
        .header(self.render_tabs(cx))
        .child(content)
        .flex_1()
        .min_h_0();
        // 右栏用 Side::Right：组件内部的边框 / 折叠动画方向朝右。
        let widget = match self.side {
            SidebarSide::Left => widget,
            SidebarSide::Right => widget.side(Side::Right),
        };

        // 这一栏自己的状态栏：默认是空条，只为与中间那条状态栏等高对齐。
        // 这一栏正在显示「会话」视图时，两端各放一枚按钮：
        // - 左下角 = 新建**文件夹**（[`NewFolder`]，落顶层）；
        // - 右下角 = 新建**会话**（[`NewSession`]，落顶层，只加记录不开终端）。
        // 标签栏的 `+` 是「直接开一个本地终端」，分工见 [`crate::main`] 模块文档。
        // 同样**不画分界边框**：列分界的那条竖线统一由分栏拖拽条来画
        // （它 `h_full` 贯穿整列，也盖住这一行；见 `crate::config::change_theme` 的说明）。
        let status_bar = StatusBar::new().h(STATUS_BAR_HEIGHT).w_full();
        let status_bar = match active {
            Some(SidebarView::Sessions) => status_bar
                .left(
                    Button::new(format!("new-folder-{}", self.side.id()))
                        .ghost()
                        .xsmall()
                        .icon(IconName::FolderPlus)
                        .tooltip("新建文件夹")
                        .on_click(cx.listener(|_, _, window, cx| {
                            window.dispatch_action(Box::new(NewFolder { parent: None }), cx);
                        })),
                )
                .right(
                    Button::new(format!("new-session-{}", self.side.id()))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Plus)
                        .tooltip("新建会话")
                        .on_click(cx.listener(|_, _, window, cx| {
                            window.dispatch_action(Box::new(NewSession { folder: None }), cx);
                        })),
                ),
            _ => status_bar,
        };

        v_flex()
            .h_full()
            .w_full()
            .overflow_hidden()
            .child(widget)
            .child(status_bar)
    }
}

/// 某一侧边栏的折叠 / 展开开关（根视图把它摆进**标题栏右端**）。
///
/// 图标与提示随折叠状态变化；放在标题栏（常驻顶部，折叠后仍点得到）。
///
/// ⚠️ 按钮必须包一层 `div().occlude()`：标题栏内容区是窗口拖拽区，不包就收不到点击。
///
/// 收 [`Entity<Sidebar>`] 而不是 `&Sidebar`：根视图的上下文是 `Context<AppRoot>`，
/// 而 `cx.listener` 需要 `Context<Sidebar>`。
pub(crate) fn toggle_button(sidebar: &Entity<Sidebar>, cx: &mut Context<AppRoot>) -> AnyElement {
    let side = sidebar.read(cx).side;
    let expanded = sidebar.read(cx).visible;
    let icon = match (side, expanded) {
        (SidebarSide::Left, true) => IconName::PanelLeftClose,
        (SidebarSide::Left, false) => IconName::PanelLeftOpen,
        (SidebarSide::Right, true) => IconName::PanelRightClose,
        (SidebarSide::Right, false) => IconName::PanelRightOpen,
    };
    let tooltip = match (side, expanded) {
        (SidebarSide::Left, true) => "折叠侧边栏",
        (SidebarSide::Left, false) => "展开侧边栏",
        (SidebarSide::Right, true) => "折叠右侧边栏",
        (SidebarSide::Right, false) => "展开右侧边栏",
    };
    let sidebar = sidebar.downgrade();
    div().occlude().child(
        Button::new(format!("sidebar-toggle-{}", side.id()))
            .ghost()
            .xsmall()
            .icon(icon)
            .tooltip(tooltip)
            .on_click(cx.listener(move |_, _, _, cx| {
                let _ = sidebar.update(cx, |sidebar, cx| sidebar.toggle(cx));
            })),
    )
    .into_any_element()
}

/// `SidebarWidget::child` 只接受单一类型，而侧边栏内容有几类（视图实体 / 内置菜单）
/// ⇒ 用一个枚举把它们的类型统一起来。
#[derive(Clone)]
enum SidebarContent {
    /// 「会话」视图的实体（它自己实现 `Render`，见 [`SessionsState`]）。
    Sessions(Entity<SessionsState>),
    /// 「文件管理器」视图的实体（它自己实现 `Render`，见 [`FilesState`]）。
    Files(Entity<FilesState>),
    /// 没有可见的标签：内容位留白（不摆任何占位文案，只留侧边栏背景）。
    Empty,
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
    /// 三个分支都不需要 `id` / `window` / `cx`(渲染与交互都在实体自己身上),故加下划线。
    fn render(
        self,
        _id: impl Into<ElementId>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> impl IntoElement {
        match self {
            // 实体自己渲染自己：这里只把它摆进侧边栏的内容位（`div` 负责给出宽度与
            // 由内容决定的高度，`SidebarItem::render` 的返回类型也才统一）。
            Self::Sessions(sessions) => div().w_full().child(sessions).into_any_element(),
            Self::Files(files) => div().w_full().child(files).into_any_element(),
            // 留白：标签条本身（固定高度）才是拖放落点，内容位不摆任何东西。
            Self::Empty => div().w_full().into_any_element(),
        }
    }
}
