//! 侧边栏(左侧 / 右侧):顶部可拖动的视图标签 + 视图内容 + 底部状态栏。
//!
//! 装配见 [`crate::AppRoot::render`]:
//! - **两侧边栏**([`AppRoot::render_sidebar_container`] /
//!   [`AppRoot::render_right_sidebar_container`]):实现是共享的 [`AppRoot::render_sidebar`],
//!   两侧只有 `Side`(边框 / 折叠方向)与各自的标签数据不同。宽度由分栏面板(可拖拽
//!   分隔条)决定,故自身只需 `w_full`;内容直接是 [`SidebarMenu`] 或会话树
//!   (**不套 `SidebarGroup`**:它固定渲染一行段标题,而标题已经在顶部标签上了)。
//! - **视图标签条**([`tabs`]):挂在 `Sidebar::header` 上 ⇒ 固定在顶部、不随内容滚动、
//!   随侧边栏折叠一起隐藏。标签可以在两条侧边栏之间自由拖动(换位 / 换面板),顺序与
//!   选中项存在 [`SidebarTabs`] 里(`AppRoot::left_tabs` / `right_tabs`)⇒
//!   **每个视图可以出现在任一侧边栏的任意位置**。
//! - **两枚折叠开关**([`AppRoot::render_sidebar_toggles`]):渲染在**标题栏右端**
//!   (见 [`crate::AppRoot::render`]),图标随各自的折叠状态变化。标题栏常驻窗口顶部,
//!   所以开关不受折叠影响、折叠后仍点得到(唯一的恢复入口)。
//! - **两条侧边状态栏**:显示「会话」视图的那条两端各一枚按钮(左下 = 新建文件夹、
//!   右下 = 新建会话),两者都**只加列表条目、不开终端**(见 [`crate::dialog`]);
//!   其余情况是空条,保留只为与中间那条状态栏等高对齐。
//!
//! 两个视图的内容各自一个文件、互不相干:
//! - [`sessions`]:会话列表(文件夹形式的记录树:增删、拖放、双击开终端);
//! - [`session_info`]:当前会话的只读信息。
//!
//! 两侧边栏用**两组嵌套的分栏面板**装配(内层 `main-split`、外层 `right-split`),
//! 因为面板宽度按下标存在 `ResizableState` 里:三个面板挤在同一组时,
//! 任一侧折叠都会让另一侧的下标漂移、拖出来的宽度丢失。
//! 「设置」入口不在本模块,而在标题栏左侧的文字按钮上(见 [`crate::AppRoot::render`])。

use gpui::{
    AnyElement, App, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, SharedString, Styled as _, Window, div, px,
};
use gpui_kit::component::{
    Collapsible, Side, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    sidebar::{Sidebar, SidebarItem, SidebarMenu, SidebarMenuItem},
    status_bar::StatusBar,
    v_flex,
};

use crate::AppRoot;
use crate::actions::{NewFolder, NewSession};
use crate::assets::IconName;
use crate::status_bar::STATUS_BAR_HEIGHT;

mod session_info;
pub(crate) mod sessions;
mod tabs;

use sessions::SessionTree;

/// 顶部视图标签条的高度(标签本身与拖拽预览卡片共用)。
pub(super) const TAB_HEIGHT: Pixels = px(24.);
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

/// 右侧边栏的名称：作它那个视图的标签文字，也作面板里的段落标题。
pub(crate) const RIGHT_SIDEBAR_LABEL: &str = "会话信息";
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
}

/// 可以停放在任一侧边栏的视图（[`SidebarTabs`] 里的元素）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SidebarView {
    /// 终端会话列表。
    Sessions,
    /// 当前会话的只读信息。
    SessionInfo,
}

impl SidebarView {
    /// 标签文字。
    fn label(self) -> &'static str {
        match self {
            Self::Sessions => "会话",
            Self::SessionInfo => RIGHT_SIDEBAR_LABEL,
        }
    }

    /// 标签提示（文字太短，鼠标停留时补一句说明）。
    fn tooltip(self) -> &'static str {
        match self {
            Self::Sessions => "终端会话",
            Self::SessionInfo => "当前会话的信息",
        }
    }

    /// 元素 id / `Sidebar` id 里用的短名。
    fn id(self) -> &'static str {
        match self {
            Self::Sessions => "sessions",
            Self::SessionInfo => "session-info",
        }
    }
}

/// 一条侧边栏上的标签：**顺序 + 当前选中**（标签可以拖到另一条侧边栏，见 [`DragSidebarTab`]）。
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

/// 拖动中的侧边栏标签（拖放载荷）。
impl AppRoot {
    /// 设置左侧边栏是否显示（只由标题栏里的折叠开关调用）。
    pub(crate) fn set_sidebar_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.sidebar_visible == visible {
            return;
        }
        self.sidebar_visible = visible;
        cx.notify();
    }

    /// 设置右侧边栏是否显示（标题栏右端的折叠开关调用）。
    pub(crate) fn set_right_sidebar_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.right_sidebar_visible == visible {
            return;
        }
        self.right_sidebar_visible = visible;
        cx.notify();
    }

    /// 两枚侧边栏折叠 / 展开开关（渲染在**标题栏右端**，见 [`crate::AppRoot::render`]）。
    ///
    /// 左枚控制左侧边栏、右枚控制右侧边栏，图标与提示随各自的折叠状态变化。
    /// 放在标题栏而不是状态栏，是因为标题栏常驻窗口顶部：侧边栏折叠后那一整块
    /// （连同它自己的状态栏）不再渲染，开关留在那里就会一起消失。
    ///
    /// ⚠️ 标题栏内容区整体是窗口拖拽区（`WindowControlArea::Drag`），其中的按钮必须包一层
    /// `div().occlude()`，否则系统把点击当成「拖标题栏」、按钮收不到（原因见
    /// [`crate::AppRoot::render`] 里的说明）。
    pub(crate) fn render_sidebar_toggles(&self, cx: &mut Context<Self>) -> AnyElement {
        let left_expanded = self.sidebar_visible;
        let right_expanded = self.right_sidebar_visible;
        h_flex()
            .items_center()
            .gap_1()
            .child(
                div().occlude().child(
                    Button::new("sidebar-toggle")
                        .ghost()
                        .xsmall()
                        .icon(if left_expanded {
                            IconName::PanelLeftClose
                        } else {
                            IconName::PanelLeftOpen
                        })
                        .tooltip(if left_expanded {
                            "折叠侧边栏"
                        } else {
                            "展开侧边栏"
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_sidebar_visible(!left_expanded, cx)
                        })),
                ),
            )
            .child(
                div().occlude().child(
                    Button::new("right-sidebar-toggle")
                        .ghost()
                        .xsmall()
                        .icon(if right_expanded {
                            IconName::PanelRightClose
                        } else {
                            IconName::PanelRightOpen
                        })
                        .tooltip(if right_expanded {
                            "折叠右侧边栏"
                        } else {
                            "展开右侧边栏"
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_right_sidebar_visible(!right_expanded, cx)
                        })),
                ),
            )
            .into_any_element()
    }

    /// 某一侧顶部的标签条（点选 / 拖动换位 / 拖到另一栏，见 [`tab_bar`]）。
    ///
    /// 挂在 `Sidebar::header` 上 ⇒ 固定在顶部（不随内容滚动），侧边栏折叠时一起隐藏。
    /// 设置入口不在这里——它是标题栏左侧的「设置」文字按钮（见 [`crate::AppRoot::render`]）。
    ///
    /// 返回 [`AnyElement`] 而非 `impl IntoElement`：本 crate 是 edition 2024，
    /// `impl Trait` 会捕获 `&mut Context` 的生命周期，导致同一渲染树里
    /// 一条侧边栏：顶部标签条 + 当前标签的内容 + 底部空状态栏。
    ///
    /// 两条侧边栏只有 `side`（`Side::Right` 决定内部边框 / 折叠动画方向）与各自的标签数据不同，
    /// 所以共用这一个实现；`Sidebar` 的 id 带上侧与当前视图，让不同视图各自记住
    /// 段落展开 / 收起状态（同一 id 会互相串）。
    ///
    /// 参考官方文档：<https://gpui-kit.com/zh-CN/component/sidebar/>
    pub(crate) fn render_sidebar(&self, side: SidebarSide, cx: &mut Context<Self>) -> AnyElement {
        let active = self.tabs(side).active_view();
        // `Sidebar::child` 只吃单一类型，而两类内容（gpui-kit 会话树 / 内置菜单）类型不同
        // ⇒ 用 [`SidebarContent`] 统一。
        let content = match active {
            Some(SidebarView::Sessions) => SidebarContent::Tree(self.render_sessions_tree()),
            Some(SidebarView::SessionInfo) => SidebarContent::Menu(self.render_session_info_menu(cx)),
            // 标签全被拖走的空标签条：给一句提示，否则整列看上去是坏的。
            None => SidebarContent::Menu(
                SidebarMenu::new().child(SidebarMenuItem::new("把标签拖到这里").disable(true)),
            ),
        };

        // 宽度由外层分栏面板决定：必须 w_full，否则 Sidebar 会回落到内置默认宽度。
        // flex_1 + min_h_0：与下方**本栏自己的状态栏**同处一列，需能收缩。
        // 空白区域**不挂**右键菜单，只有会话条目有自己的右键菜单。
        let sidebar = Sidebar::new(SharedString::from(format!(
            "sidebar-{}-{}",
            side.id(),
            active.map_or("empty", SidebarView::id)
        )))
        .w_full()
        // 顶部固定的标签条（不随下方内容滚动）。
        .header(self.render_view_tabs(side, cx))
        .child(content)
        .flex_1()
        .min_h_0();
        // 右栏用 Side::Right：组件内部的边框 / 折叠动画方向朝右。
        let sidebar = match side {
            SidebarSide::Left => sidebar,
            SidebarSide::Right => sidebar.side(Side::Right),
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
                    Button::new(format!("new-folder-{}", side.id()))
                        .ghost()
                        .xsmall()
                        .icon(IconName::FolderPlus)
                        .tooltip("新建文件夹")
                        .on_click(cx.listener(|_, _, window, cx| {
                            window.dispatch_action(Box::new(NewFolder { parent: None }), cx);
                        })),
                )
                .right(
                    Button::new(format!("new-session-{}", side.id()))
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
            .child(sidebar)
            .child(status_bar)
            .into_any_element()
    }
    /// 左侧边栏容器（见 [`AppRoot::render_sidebar`]）。
    pub(crate) fn render_sidebar_container(&self, cx: &mut Context<Self>) -> AnyElement {
        self.render_sidebar(SidebarSide::Left, cx)
    }

    /// 右侧边栏容器（见 [`AppRoot::render_sidebar`]）。
    pub(crate) fn render_right_sidebar_container(&self, cx: &mut Context<Self>) -> AnyElement {
        self.render_sidebar(SidebarSide::Right, cx)
    }
}
/// `Sidebar::child` 只接受单一类型，而侧边栏内容有两类（gpui-kit 会话树 / 内置菜单）
/// ⇒ 用一个枚举把它们的类型统一起来。
#[derive(Clone)]
enum SidebarContent {
    /// 文件夹形式的会话树。
    Tree(SessionTree),
    /// 内置菜单（会话信息 / 空标签条提示）。
    Menu(SidebarMenu),
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
    fn render(
        self,
        id: impl Into<ElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement {
        match self {
            Self::Tree(tree) => tree.render(id, window, cx).into_any_element(),
            Self::Menu(menu) => menu.render(id, window, cx).into_any_element(),
        }
    }
}
