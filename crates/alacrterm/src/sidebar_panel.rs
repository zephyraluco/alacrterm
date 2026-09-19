//! 侧边栏（左侧 / 右侧）：顶部可拖动的视图标签 + 内容 + 底部空状态栏。
//!
//! 装配见 [`crate::AppRoot::render`]：
//! - **两侧边栏**（[`AppRoot::render_sidebar_container`] /
//!   [`AppRoot::render_right_sidebar_container`]）：实际实现是共享的
//!   [`AppRoot::render_sidebar`]，两侧只有 `Side`（边框 / 折叠方向）与各自的标签数据不同。
//!   宽度由分栏面板（可拖拽分隔条）决定，故自身只需 `w_full`。
//!   内容直接是 [`SidebarMenu`]（**不套 `SidebarGroup`**：它固定渲染一行段标题，
//!   标题已在顶部标签上了，套上就是白占 32px 高的空行）。
//! - **视图标签条**（[`AppRoot::render_view_tabs`]）：挂在 `Sidebar::header` 上 ⇒
//!   固定在顶部不随内容滚动、随侧边栏折叠一起隐藏。外观照 VS Code 的面板标签
//!   （纯文字、无边框、按内容宽度左对齐，见 [`tab_element`]）。
//!   **标签可以拖动**：在同一栏内拖动 = 换位，拖到另一栏 = 把视图搬过去
//!   （载荷 [`DragSidebarTab`]，放下后目标栏选中它）；两侧的标签顺序与选中项存在
//!   [`SidebarTabs`] 里（`AppRoot::left_tabs` / `right_tabs`），所以**每个视图可以
//!   出现在任一侧边栏的任意位置**。
//! - **两枚折叠开关**（[`AppRoot::render_sidebar_toggles`]）：渲染在**标题栏右端**
//!   （见 [`crate::AppRoot::render`]），图标随各自的折叠状态变化。标题栏常驻窗口顶部，
//!   因此开关不受侧边栏折叠影响，折叠后仍点得到（唯一的恢复入口）。
//! - **两条侧边状态栏当前都是空条**（内容已全部移到顶部标签），
//!   保留空条只为与中间那条状态栏等高对齐。
//!
//! 两侧边栏用**两组嵌套的分栏面板**装配（内层 `main-split`、外层 `right-split`），
//! 因为面板宽度按下标存在 `ResizableState` 里：三个面板挤在同一组时，
//! 任一侧折叠都会让另一侧的下标漂移、拖出来的宽度丢失。
//! 「设置」入口不在本模块，而在标题栏左侧的文字按钮上（见 [`crate::AppRoot::render`]）。

use gpui::{
    AnyElement, App, AppContext as _, Context, CursorStyle, ElementId, Entity,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_kit::component::{
    ActiveTheme as _, Collapsible, Icon, Side, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    list::ListItem,
    sidebar::{Sidebar, SidebarItem, SidebarMenu, SidebarMenuItem},
    status_bar::StatusBar,
    tree::{TreeEntry, TreeItem, TreeState, tree},
    v_flex,
};

use crate::actions::{CloseSession, NewTerminal};
use crate::AppRoot;
use crate::assets::IconName;
use crate::status_bar::STATUS_BAR_HEIGHT;

/// 侧边栏默认宽度（分栏面板首次布局时的初始宽度）。
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

/// 标签条高度（与之前 `Toggle::small()` 的高度一致）。
const TAB_HEIGHT: Pixels = px(24.);
/// 会话树的每行高度：`Tree` 内部的虚拟列表是**等高**的，所以这个值也得用来算整棵树的高度。
const TREE_ROW_HEIGHT: Pixels = px(28.);
/// 标签之间的缝：主要间距来自各自的左右内边距。
const TAB_GAP: Pixels = px(2.);
/// 标签左右内边距。
const TAB_PADDING_X: Pixels = px(8.);

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
#[derive(Clone)]
struct DragSidebarTab {
    /// 来自哪一侧。
    side: SidebarSide,
    /// 在来源侧标签条里的下标（按下标取标签，避免同名视图重定位）。
    index: usize,
}

/// 拖标签时跟着鼠标的小卡片。
struct TabDragPreview {
    label: SharedString,
}

impl Render for TabDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .h(TAB_HEIGHT)
            .flex()
            .items_center()
            .rounded(cx.theme().radius)
            .bg(cx.theme().tokens.accent)
            .text_color(cx.theme().accent_foreground)
            .child(self.label.clone())
    }
}

/// 标签条：一条侧边栏顶部那一行标签（点选 / 拖动换位 / 拖到另一栏）。
///
/// 外观照 **VS Code 的面板标签**（问题 / 输出 / 调试控制台 / 终端 / 端口）：纯文字、
/// 无边框、按内容宽度左对齐，只有悬停 / 选中的那块有圆角浅灰底
/// （`tokens.accent` 底 + `accent_foreground` 字，圆角用主题 `radius`）。
/// 只有一个标签也照画：标签条不因只有一项而隐藏（右侧边栏默认就是一项）。
///
/// 拖放：每个标签自己既是拖源也是落点（落在第 `ix` 个标签上 = 占它的位置）；
/// 标签条本身的空白区域（含空标签条）是**兜底落点** = 追加到末尾。
fn tab_bar(side: SidebarSide, tabs: &SidebarTabs, cx: &mut Context<AppRoot>) -> AnyElement {
    // 先收成 `Vec`：下面还要用 `cx.listener` 借一次 `cx`，迭代器会一直绕着它。
    let items: Vec<AnyElement> = tabs
        .views
        .iter()
        .enumerate()
        .map(|(ix, view)| tab_element(side, ix, *view, ix == tabs.active, cx))
        .collect();

    h_flex()
        .w_full()
        // 必须显式给高度：标签条可能一个标签都没有（都被拖走），
        // 没有子元素时 flex 容器的高度会塌成 0 ⇒ 兜底落点根本悬停不到。
        .h(TAB_HEIGHT)
        .gap(TAB_GAP)
        // 兜底落点：拖到标签右侧的空白（或目标栏为空时只能拖到这里）⇒ 追加到末尾。
        .on_drop(cx.listener(move |this, drag: &DragSidebarTab, _, cx| {
            this.append_sidebar_tab(drag, side, cx)
        }))
        .children(items)
        .into_any_element()
}

/// 一个标签：点选 + 可拖动（拖到本栏另一个标签上 = 换位，拖到另一栏 = 换面板）。
///
/// 左/右两条标签条共用；`side` 与 `index` 是拖放载荷的来源信息（见 [`DragSidebarTab`]）。
fn tab_element(
    side: SidebarSide,
    index: usize,
    view: SidebarView,
    selected: bool,
    cx: &mut Context<AppRoot>,
) -> AnyElement {
    let (accent, accent_fg, fg, radius) = {
        let theme = cx.theme();
        (
            theme.tokens.accent,
            theme.accent_foreground,
            theme.tab_foreground,
            theme.radius,
        )
    };

    h_flex()
        .id(format!("sidebar-tab-{}", view.id()))
        .flex_none()
        .h(TAB_HEIGHT)
        .px(TAB_PADDING_X)
        .items_center()
        .rounded(radius)
        // 常驻一条透明左边框：拖动悬停时把它点亮成插入位置提示，
        // 这样高亮不会把标签尺寸顶变（否则整条标签会在拖动时抽动）。
        .border_l_2()
        .border_color(accent.opacity(0.0))
        .cursor(CursorStyle::PointingHand)
        .text_color(if selected { accent_fg } else { fg })
        // 标签文字很短，给读屏 / 无障碍一个完整说明。
        .aria_label(view.tooltip())
        .when(selected, |this| this.bg(accent))
        .when(!selected, |this| {
            this.hover(|style| style.bg(accent).text_color(accent_fg))
        })
        .on_click(cx.listener(move |this, _, _, cx| {
            this.select_sidebar_tab(side, index, cx)
        }))
        .on_drag(DragSidebarTab { side, index }, move |_, _, _, cx| {
            cx.new(|_| TabDragPreview {
                label: view.label().into(),
            })
        })
        .drag_over::<DragSidebarTab>(move |style, _, _, _| {
            style.border_l_2().border_color(accent)
        })
        .on_drop(
            cx.listener(move |this, drag: &DragSidebarTab, _, cx| {
                this.drop_sidebar_tab(drag, side, index, cx)
            }),
        )
        .child(view.label())
        .into_any_element()
}

impl AppRoot {
    /// 某一侧的标签数据。
    fn tabs(&self, side: SidebarSide) -> &SidebarTabs {
        match side {
            SidebarSide::Left => &self.left_tabs,
            SidebarSide::Right => &self.right_tabs,
        }
    }

    fn tabs_mut(&mut self, side: SidebarSide) -> &mut SidebarTabs {
        match side {
            SidebarSide::Left => &mut self.left_tabs,
            SidebarSide::Right => &mut self.right_tabs,
        }
    }

    /// 让某一侧边栏可见（切视图 / 拖动落地时都意味着「用户在看它」）。
    fn show_sidebar(&mut self, side: SidebarSide) {
        match side {
            SidebarSide::Left => self.sidebar_visible = true,
            SidebarSide::Right => self.right_sidebar_visible = true,
        }
    }

    /// 点击标签：切到它代表的视图。
    ///
    /// **只切视图，不改可见性**——折叠 / 展开只由标题栏右端的折叠开关负责，
    /// 与右侧边栏一致（那边除折叠开关外没有任何按钮会改可见性）。
    /// 因此点击**当前**标签是空操作。
    fn select_sidebar_tab(&mut self, side: SidebarSide, index: usize, cx: &mut Context<Self>) {
        if !self.tabs_mut(side).select(index) {
            return;
        }
        // 标签只在侧边栏可见时渲染，所以这里其实必然是 true；
        // 保留赋值是为了让「切视图 ⇒ 侧边栏可见」这个不变量不依赖渲染条件。
        self.show_sidebar(side);
        cx.notify();
    }

    /// 把拖动的标签放到 `target` 栏的第 `index` 个标签处（占它的位置）。
    ///
    /// `index` 是**拖动前**目标栏里的下标：先在目标栏插入、再选中它，
    /// 所以「拖到第 i 个标签上」的结果就是「这个视图落在第 i 个位置、其余顺延」。
    /// 同一栏内拖动也是同一套语义（先抽走再按原下标插入），因此
    /// 「把 A 拖到 B 上」在 `[A, B]` 这种只有两项时也能得到 `[B, A]`。
    fn drop_sidebar_tab(
        &mut self,
        drag: &DragSidebarTab,
        target: SidebarSide,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.tabs_mut(drag.side).take(drag.index) else {
            return;
        };
        self.tabs_mut(target).insert_active(index, view);
        self.show_sidebar(target);
        cx.notify();
    }

    /// 追加到 `target` 栏末尾（拖到标签条空白处，或目标栏为空时的兑现落点）。
    fn append_sidebar_tab(
        &mut self,
        drag: &DragSidebarTab,
        target: SidebarSide,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.tabs_mut(drag.side).take(drag.index) else {
            return;
        };
        self.tabs_mut(target).insert_active(usize::MAX, view);
        self.show_sidebar(target);
        cx.notify();
    }

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
    /// 连续调用多个 `&mut cx` 的渲染方法时借用冲突；装箱可彻底规避。
    pub(crate) fn render_view_tabs(&self, side: SidebarSide, cx: &mut Context<Self>) -> AnyElement {
        tab_bar(side, self.tabs(side), cx)
    }

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
            Some(SidebarView::Sessions) => SidebarContent::Tree(self.render_sessions_tree(cx)),
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

        // 这一栏自己的状态栏：**当前不放任何内容**，保留空条只为与中间那条等高对齐
        // （前者本来就是全程序唯一一条共用状态栏，见 [`crate::status_bar`]）。
        // 同样**不画分界边框**：列分界的那条竖线统一由分栏拖拽条来画
        // （它 `h_full` 贯穿整列，也盖住这一行；见 `crate::change_theme` 的说明）。
        let status_bar = StatusBar::new().h(STATUS_BAR_HEIGHT).w_full();

        v_flex()
            .h_full()
            .w_full()
            .overflow_hidden()
            .child(sidebar)
            .child(status_bar)
            .into_any_element()
    }

    /// 「会话」视图的内容：gpui-kit [`tree`]（见 [`SessionTree`]）。
    fn render_sessions_tree(&self, cx: &Context<Self>) -> SessionTree {
        // 行数 = 文件夹行 + 展开时的会话行。
        // ⚠️ `Tree` 内部是**虚拟列表**（等高），不会被内容撑高，而它又是塞在
        // `Sidebar` 自己的虚拟列表里的一个自动高度 item ⇒ 必须自己把高度算出来。
        let rows = 1 + if self.session_tree_root.is_expanded() {
            self.session_tree_root.children.len()
        } else {
            0
        };
        SessionTree {
            state: self.session_tree.clone(),
            root: cx.entity().downgrade(),
            height: TREE_ROW_HEIGHT * rows as f32,
        }
    }

    /// 把会话表同步进 `TreeState`（在 [`crate::AppRoot::render`] 开头调用）。
    ///
    /// 三个要点：
    /// 1. `TreeState::set_items` 会 `notify` ⇒ **不能每帧无条件调用**，这里靠内容签名
    ///    （标题 / 当前会话 / 数量）挡一下，否则自激成死循环；
    /// 2. `set_items` 会清掉选中项，所以重建后要把「当前会话那一行」重新选上
    ///    （行下标 = 会话下标 + 1，0 号是文件夹行）。**选中项同时充当高亮来源**；
    /// 3. 选中项还会被**行点击**改掉：`TreeState` 的行点击一律 `selected_ix = ix`
    ///    （点文件夹行也一样，虽然它展开 / 收起是对的，但选中项就被抢走了）。
    ///    选中项是我们唯一的「当前会话」标记，所以每帧发现不对就掰回来。
    pub(crate) fn sync_session_tree(&mut self, cx: &mut Context<Self>) {
        let sig: Vec<(SharedString, bool)> = self
            .terminals
            .iter()
            .enumerate()
            // 显示名：用户在建连对话框里填写的名称优先，否则终端自身标题。
            .map(|(ix, session)| (session.title(cx), ix == self.active))
            .collect();
        let selected = sig.iter().position(|(_, active)| *active).map(|ix| ix + 1);

        if sig == self.session_tree_sig {
            if self.session_tree.read(cx).selected_index() != selected {
                self.session_tree
                    .update(cx, |state, cx| state.set_selected_index(selected, cx));
            }
            return;
        }

        self.session_tree_sig = sig;
        // 复用同一个根项：展开状态存在 `TreeItem` 内部那个**共享的** `Rc<RefCell<..>>` 上，
        // `set_items` 只重建扁平表 ⇒ 用户的展收状态不会被抹掉。
        self.session_tree_root.label = format!("{} 个会话", self.session_tree_sig.len()).into();
        self.session_tree_root.children = self
            .session_tree_sig
            .iter()
            .enumerate()
            .map(|(ix, (title, _))| TreeItem::new(format!("session-{ix}"), title.clone()))
            .collect();

        let root = self.session_tree_root.clone();
        self.session_tree.update(cx, |state, cx| {
            state.set_items([root], cx);
            state.set_selected_index(selected, cx);
        });
    }

    /// 「会话信息」视图的内容：当前会话的只读信息（名称 / 连接 / 进程 / 状态）。
    ///
    /// 无会话（标签页全部关闭）时四项都显示 `--`，避免面板看上去是空的。
    fn render_session_info_menu(&self, cx: &mut Context<Self>) -> SidebarMenu {
        let (name, target, pid, state) = match self.terminals.get(self.active) {
            Some(session) => {
                let view = session.view.read(cx);
                let pid = view
                    .pid(cx)
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "--".to_string());
                let state = if view.has_exited() {
                    "已断开"
                } else {
                    "运行中"
                };
                (
                    session.title(cx),
                    session.target(cx).label(),
                    pid,
                    state.to_string(),
                )
            }
            None => (
                SharedString::from("--"),
                "--".to_string(),
                "--".to_string(),
                "无会话".to_string(),
            ),
        };

        SidebarMenu::new().child(
            SidebarMenuItem::new("当前会话")
                .icon(IconName::SquareTerminal)
                .default_open(true)
                .click_to_toggle(true)
                .children([
                    SidebarMenuItem::new(format!("名称：{name}")).disable(true),
                    SidebarMenuItem::new(format!("连接：{target}")).disable(true),
                    SidebarMenuItem::new(format!("进程：{pid}")).disable(true),
                    SidebarMenuItem::new(format!("状态：{state}")).disable(true),
                ]),
        )
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

/// 「会话」视图的内容：gpui-kit 的 [`tree`]（**文件夹形式的树**）。
///
/// 树的数据与交互状态（展开、选中、滚动、键盘导航、无障碍 role）都在
/// `AppRoot::session_tree`（`Entity<TreeState>`）里，由 [`AppRoot::sync_session_tree`]
/// 同步；这里只负责两件事：按行数把高度算出来、以及「点会话行怎么切过去」。
///
/// ⚠️ `Tree` 内部是**虚拟列表 + `size_full()`**，而它是塞在 `Sidebar` 自己的虚拟列表里的
/// 一个自动高度 item（`Sidebar::child` 要 `SidebarItem`）⇒ 拿不到确定高度、高度会塌成 0。
/// 所以必须 `.h(行数 × 行高)` 手动给高度（`Tree` 的 `refine_style` 在链尾，能盖住 `size_full`）。
#[derive(Clone)]
pub(crate) struct SessionTree {
    /// 树的状态（由 `AppRoot` 持有，跨帧复用）。
    state: Entity<TreeState>,
    /// 行回调要用的根视图：`SidebarItem::render` 只拿得到 `&mut App`，用不了 `cx.listener`。
    root: WeakEntity<AppRoot>,
    /// 整棵树的高度（行数 × [`TREE_ROW_HEIGHT`]）。
    height: Pixels,
}

impl Collapsible for SessionTree {
    fn is_collapsed(&self) -> bool {
        false
    }

    fn collapsed(self, _: bool) -> Self {
        self
    }
}

impl SidebarItem for SessionTree {
    fn render(
        self,
        _id: impl Into<ElementId>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> impl IntoElement {
        let root = self.root.clone();
        tree(&self.state, move |ix, entry, selected, _window, cx| {
            session_tree_row(ix, entry, selected, &root, cx)
        })
        .h(self.height)
        // 右键菜单由树统一挂：文件夹行不弹，会话行还是「关闭会话 / 新建终端」。
        .context_menu(move |_ix, entry, menu, _, _| {
            let Some(session_ix) = session_index_of(entry) else {
                return menu;
            };
            menu.menu("关闭会话", Box::new(CloseSession { index: session_ix }))
                .separator()
                .menu("新建终端", Box::new(NewTerminal))
        })
    }
}

/// 从树行的 id 里取会话下标（id 形如 `session-{ix}`；文件夹行返回 `None`）。
///
/// 比「行号减一」稳：行号会随展开 / 收起变化，id 不会。
fn session_index_of(entry: &TreeEntry) -> Option<usize> {
    entry.item().id.strip_prefix("session-")?.parse().ok()
}

/// 会话树的一行。
///
/// - **根行 / 文件夹行**（`entry.is_root() || entry.is_folder()`）：
///   文件夹图标 + 「N 个会话」，展开 / 收起交给 `TreeState`
///   （它自己处理行点击：`on_entry_click` → `toggle_expand`）；
///   ⚠️ 「0 个会话」时根项没有子项，`is_folder()` 为假——所以还要看 `is_root()`，
///   否则它会掉进下面的会话行分支（被缩进、带个终端图标）；
/// - **会话行**：缩进 + 终端图标 + 会话名，点它切到该会话；高亮用 `selected`
///   ——「当前会话」那一行由 [`AppRoot::sync_session_tree`] 同步成树的选中项，
///   底色由 `ListItem` 统一画（`accent`，见 [`crate::change_theme`] 里关掉
///   `list.active_highlight` 的原因），文字色配 `sidebar_accent_foreground`。
fn session_tree_row(
    ix: usize,
    entry: &TreeEntry,
    selected: bool,
    root: &WeakEntity<AppRoot>,
    cx: &mut App,
) -> ListItem {
    let (radius, accent_fg) = {
        let theme = cx.theme();
        (theme.radius, theme.sidebar_accent_foreground)
    };
    let label = entry.item().label.clone();

    if entry.is_folder() || entry.is_root() {
        let icon = if entry.is_expanded() {
            IconName::FolderOpen
        } else {
            IconName::Folder
        };
        let caret = if entry.is_expanded() {
            IconName::ChevronDown
        } else {
            IconName::ChevronRight
        };
        return ListItem::new(ix)
            .h(TREE_ROW_HEIGHT)
            .px_2()
            .rounded(radius)
            .text_sm()
            .child(
                h_flex()
                    .gap_x_2()
                    .items_center()
                    // 没有子项时不给 caret（点了也不会有反应）。
                    .when(entry.is_folder(), |this| this.child(Icon::new(caret).size_3()))
                    .child(Icon::new(icon).size_3())
                    .child(label),
            );
    }

    let root = root.clone();
    ListItem::new(ix)
        .h(TREE_ROW_HEIGHT)
        // 子项缩进：对齐到文件夹「文字」那一列（px_2 + caret + gap + 文件夹图标 + gap）。
        .pl(px(46.))
        .pr_2()
        .rounded(radius)
        .text_sm()
        .overflow_x_hidden()
        .cursor(CursorStyle::PointingHand)
        .when(selected, |this| this.font_medium().text_color(accent_fg))
        .map(|this| match session_index_of(entry) {
            Some(session_ix) => this.on_click(move |_, window, cx| {
                let _ = root.update(cx, |root, cx| root.set_active_tab(session_ix, window, cx));
            }),
            // id 不是 `session-*`：不该出现（根行已在上面拦掉），不挂点击总比乱切会话好。
            None => this,
        })
        .child(
            h_flex()
                .gap_x_2()
                .items_center()
                .child(Icon::new(IconName::SquareTerminal).size_3())
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(label),
                ),
        )
}


