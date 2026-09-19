//! 侧边栏（左侧 / 右侧）与它们的折叠开关。
//!
//! 两侧边栏 + 视图段控 + 开关分处两个模块，装配见 [`crate::AppRoot::render`]：
//! - **左侧边栏**（[`AppRoot::render_sidebar_container`]）：宽度由分栏面板（可拖拽分隔条）
//!   决定，故自身只需 `w_full`。内容为「[`SidebarGroup`] 段落标题 + 段内可折叠菜单」
//!   两层结构：段落标题给出分区（会话 / 关于 / 版本信息），段内菜单项是官方
//!   「嵌套菜单」形态，点击表头行即展开 / 收起下方的会话列表。
//! - **右侧边栏**（[`AppRoot::render_right_sidebar_container`]）：终端右侧的面板，
//!   当前展示当前会话的只读信息（名称 / 连接 / 进程 / 状态）；用 `Side::Right` 构造，
//!   与左侧对称。
//! - **两枚折叠开关**（[`AppRoot::render_sidebar_toggles`]）：渲染在**标题栏右端**
//!   （见 [`crate::AppRoot::render`]），图标随各自的折叠状态变化。标题栏常驻窗口顶部，
//!   因此开关不受侧边栏折叠影响，折叠后仍点得到（唯一的恢复入口）。
//! - **视图切换栏**（[`AppRoot::render_view_tabs`] / [`AppRoot::render_right_sidebar_tabs`]）：
//!   两条侧边栏**顶部**的 segmented tabs（gpui-kit `TabBar::segmented()`，
//!   选中态是滑动的圆角药丸），都挂在各自的 `Sidebar::header` 上 ⇒
//!   固定不滚动、随侧边栏折叠一起隐藏。左栏两段（会话 / 关于）切视图、只切视图；
//!   右栏目前**只有一段**（会话信息），但**单段也照画**（[`view_tabs`]），
//!   将来加第二块内容时不用再改布局。
//! - **两条侧边状态栏当前为空**：左栏原放视图图标、右栏原放「会话信息」标识，
//!   两边的内容都已移走（图标→顶部段控），保留空条只为与中间状态栏等高对齐。
//!
//! 两侧边栏用**两组嵌套的分栏面板**装配（内层 `main-split`、外层 `right-split`），
//! 因为面板宽度按下标存在 `ResizableState` 里：三个面板挤在同一组时，
//! 任一侧折叠都会让另一侧的下标漂移、拖出来的宽度丢失。
//! 「设置」入口不在本模块，而在标题栏左侧的文字按钮上（见 [`crate::AppRoot::render`]）。

use gpui::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    SharedString, Styled as _, Window, div, px,
};
use gpui_kit::component::{
    Side, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    sidebar::{Sidebar, SidebarGroup, SidebarMenu, SidebarMenuItem},
    status_bar::StatusBar,
    tab::{Tab, TabBar},
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

/// 右侧边栏的名称：既作面板里的段落标题，也作它顶部那一段 tab 的文字。
pub(crate) const RIGHT_SIDEBAR_LABEL: &str = "会话信息";

/// 侧边栏顶部的视图切换栏（两条侧边栏共用同一套外观）。
///
/// 用 gpui-kit 内置的 `TabBar::segmented()`（即 libadwaita view switcher 那套观感）：
/// 「槽」底色、左右内边距与段间距都由它自己算（`tokens.tab_bar_segmented` /
/// `padding_x` / `gap: px(2.)`），选中态是一个**会滑动**的圆角药丸
/// （`tokens.background` + `raised_shadow()`，spring 动画）⇒ 我们不需要再画任何
/// 边框、分隔线或选中底色。
///
/// `tabs` 只有一项也照画：**切换栏不因只有一段而隐藏**（右侧边栏目前就是一段）。
///
/// ⚠️ `Tab` 的元素 id 用的是下标整数（`ElementId` 是全局 Id 的**路径**，按祖先分层，
/// 不会与其它树里的同名下标撞车），所以同一个 `TabBar` 内不要重复下标的 Tab。
fn view_tabs(
    id: &'static str,
    selected: usize,
    tabs: Vec<Tab>,
    on_click: impl Fn(&usize, &mut Window, &mut App) + 'static,
) -> AnyElement {
    TabBar::new(id)
        .segmented()
        .small()
        // 铺满侧边栏宽度；各段自己 `.flex_1()`，靠 TabBar 把
        // `flex_grow` / `flex_basis` 转给它的包装层 ⇒ 等宽平分。
        .w_full()
        .selected_index(selected)
        .on_click(on_click)
        .children(tabs)
        .into_any_element()
}

/// 侧边栏视图（对应顶部的视图切换栏）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SidebarView {
    /// 终端会话列表。
    Sessions,
    /// 关于。
    About,
}

impl AppRoot {
    /// 视图段控点击：切换左栏视图。
    ///
    /// **只切视图，不改可见性**——折叠 / 展开只由标题栏右端的折叠开关负责，
    /// 与右侧边栏一致（那边除折叠开关外没有任何按钮会改可见性）。
    /// 因此点击**当前**视图图标是空操作（图标本来就只在侧边栏可见时渲染）。
    pub(crate) fn set_sidebar_view(&mut self, view: SidebarView, cx: &mut Context<Self>) {
        if self.sidebar_view == view {
            return;
        }
        self.sidebar_view = view;
        // 图标只在侧边栏可见时渲染，所以这里其实必然是 true；
        // 保留赋值是为了让「切视图 ⇒ 侧边栏可见」这个不变量不依赖渲染条件。
        self.sidebar_visible = true;
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

    /// 侧边栏顶部的视图切换栏（segmented tabs）：在「终端会话 / 关于」之间切换。
    ///
    /// 挂在 `Sidebar::header` 上：固定在侧边栏顶部（不随内容滚动），侧边栏折叠时随之隐藏。
    /// 这两个入口原先在左栏状态栏里（两枚图标按钮），现已整体迁到这里。
    /// 点击**只切视图**（[`AppRoot::set_sidebar_view`]）：不会折叠 / 展开侧边栏
    ///（`TabBar` 的选中态完全由 `selected_index` 控制，点已选中的那段是空操作）。
    /// 设置入口不在这里——它是标题栏左侧的「设置」文字按钮（见 [`crate::AppRoot::render`]）。
    ///
    /// 返回 [`AnyElement`] 而非 `impl IntoElement`：本 crate 是 edition 2024，
    /// `impl Trait` 会捕获 `&mut Context` 的生命周期，导致同一渲染树里
    /// 连续调用多个 `&mut cx` 的渲染方法时借用冲突；装箱可彻底规避。
    pub(crate) fn render_view_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        view_tabs(
            "sidebar-view-tabs",
            match self.sidebar_view {
                SidebarView::Sessions => 0,
                SidebarView::About => 1,
            },
            vec![
                // ⚠️ 不要给 `Tab` 设 icon：gpui-kit 的 `Tab` 在 `icon` 为 Some 时
                // **只画图标、整个丢掉 label**（见 `tab.rs` 的 inner_content 分支），
                // 所以想要「图标 + 文字」就不能用 `Tab`。这里照参考图做纯文字。
                Tab::new().label("会话").flex_1(),
                Tab::new().label("关于").flex_1(),
            ],
            // 只有两段，按回调给的下标切即可；点当前那段会被 `set_sidebar_view` 忽略。
            cx.listener(|this, ix: &usize, _, cx| {
                let next = if *ix == 0 {
                    SidebarView::Sessions
                } else {
                    SidebarView::About
                };
                this.set_sidebar_view(next, cx);
            }),
        )
    }

    /// 右侧边栏顶部的视图切换栏。
    ///
    /// 右栏目前**只有一个视图**（[`RIGHT_SIDEBAR_LABEL`]），但照画不误：
    /// 切换栏不因只有一段而隐藏（[`view_tabs`]），将来加第二块内容时不用再改布局，
    /// 两条侧边栏的顶部也保持一致。
    pub(crate) fn render_right_sidebar_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        view_tabs(
            "right-sidebar-view-tabs",
            0,
            vec![Tab::new().label(RIGHT_SIDEBAR_LABEL).flex_1()],
            // 只有一个视图：点它不做任何事。
            cx.listener(|_, _: &usize, _, _| {}),
        )
    }

    /// 侧边栏容器：会话列表（或「关于」）+ 底部状态栏。
    ///
    /// 参考官方文档：<https://gpui-kit.com/zh-CN/component/sidebar/>
    pub(crate) fn render_sidebar_container(&self, cx: &mut Context<Self>) -> AnyElement {
        // 侧边栏菜单项：每个终端会话一项，点击切换激活会话，右键弹出上下文菜单。
        //
        // 菜单项用官方写法 `menu.menu(标签, Box::new(Action))`，点击后由菜单
        // `dispatch_action` 派发，统一由 `actions.rs` 里的全局监听器接管。
        // 注意：目前只有**条目**有右键菜单，侧边栏空白区域不弹菜单。
        let this = cx.entity().downgrade();
        let items = self.terminals.iter().enumerate().map(|(ix, session)| {
            let this = this.clone();
            // 显示名：用户在建连对话框里填写的名称优先，否则终端自身标题。
            SidebarMenuItem::new(session.title(cx))
                .icon(IconName::SquareTerminal)
                .active(ix == self.active)
                .on_click(move |_, window, cx| {
                    let _ = this.update(cx, |this, cx| this.set_active_tab(ix, window, cx));
                })
                .context_menu(move |menu, _, _| {
                    menu.menu("关闭会话", Box::new(CloseSession { index: ix }))
                        .separator()
                        .menu("新建终端", Box::new(NewTerminal))
                })
        });

        // 侧边栏内容：随顶部段控选中的视图切换。
        //
        // 结构统一为「`SidebarGroup` 段落标题 + 段内菜单」，菜单项本身是
        // 「带子项的可折叠菜单项」（官方嵌套菜单形态）：表头行右侧有 caret，
        // 点击表头行或 caret 都能展开 / 收起；展开状态由组件内部按 element id
        // 存在 keyed state 里，应用无需自己维护。
        // 注意 `SidebarGroup` 自身不负责折叠（它只渲染段落标题 + 子项），
        // 折叠能力由段内的菜单项提供。
        // 两个分支的类型统一为 `SidebarGroup<SidebarMenu>`（match 各分支类型必须一致）。
        let content = match self.sidebar_view {
            SidebarView::Sessions => {
                let session_count = self.terminals.len();
                SidebarGroup::new("会话").child(
                    SidebarMenu::new().child(
                        // 标签带会话数量：收起后也能一眼看出有多少个会话。
                        SidebarMenuItem::new(format!("{session_count} 个会话"))
                            .icon(IconName::SquareTerminal)
                            .default_open(true)
                            // 点整行即切换展开状态（表头行本身没有导航行为）。
                            .click_to_toggle(true)
                            .children(items),
                    ),
                )
            }
            SidebarView::About => SidebarGroup::new("关于").child(
                SidebarMenu::new().child(
                    SidebarMenuItem::new("版本信息")
                        .icon(IconName::Info)
                        .default_open(true)
                        .click_to_toggle(true)
                        .children([
                            SidebarMenuItem::new("test-rs 终端").disable(true),
                            SidebarMenuItem::new("gpui-kit 0.6 · gpui-pre 0.3").disable(true),
                            SidebarMenuItem::new("alacritty_terminal 0.26 · ConPTY").disable(true),
                        ]),
                ),
            ),
        };

        // Sidebar 的 id 随视图变化：折叠状态按 element id 存在 keyed state 里，
        // 若两个视图共用同一个 id（内容都只有 1 个顶层项，index 恒为 0），
        // 收起「会话」后切到「关于」也会呈收起状态——分 id 即可各自独立。
        let sidebar_id = match self.sidebar_view {
            SidebarView::Sessions => "sidebar-sessions",
            SidebarView::About => "sidebar-about",
        };

        // 宽度由外层分栏面板决定：必须 w_full，否则 Sidebar 会回落到内置默认宽度。
        // flex_1 + min_h_0：与下方**本栏自己的状态栏**同处一列，需能收缩。
        // 空白区域**不挂**右键菜单，只有会话条目有自己的右键菜单。
        let sidebar = Sidebar::new(sidebar_id)
            .w_full()
            // 顶部固定的视图段控（不随下方内容滚动）。
            .header(self.render_view_tabs(cx))
            .child(content)
            .flex_1()
            .min_h_0();

        // 左栏自己的状态栏：**当前不放任何内容**（视图切换已移到顶部段控）。
        // 保留这条空条是为了与中间 / 右栏那两条等高对齐（三条底边在同一条线上）；
        // 同样**不画右边框**：列分界的那条竖线统一由分栏拖拽条来画
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

    /// 右侧边栏容器：当前会话的只读信息（名称 / 连接 / 进程 / 状态）。
    ///
    /// 内容刻意保持精简——它是右侧边栏的第一个面板，后续可替换成缓冲区列表、
    /// 输出日志等。宽度由外层分栏面板（`right-split`）决定，所以自身只需 `w_full`；
    /// `side(Side::Right)` 让组件内部的边框 / 折叠动画方向朝右。
    pub(crate) fn render_right_sidebar_container(&self, cx: &mut Context<Self>) -> AnyElement {
        // 只读信息：取当前会话的显示名 / 连接目标 / 进程号 / 运行状态。
        // 无会话（标签页全部关闭）时四项都显示 `--`，避免面板看上去是空的。
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

        let content = SidebarGroup::new(RIGHT_SIDEBAR_LABEL).child(
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
            ),
        );

        // 与左侧边栏一致：宽度随分栏面板（w_full），状态栏在下面一起撑开。
        // 空白区域不挂右键菜单。
        let sidebar = Sidebar::new("sidebar-right")
            .side(Side::Right)
            .w_full()
            // 顶部固定的视图 tab（当前只有一段，照画）。
            .header(self.render_right_sidebar_tabs(cx))
            .child(content)
            .flex_1()
            .min_h_0();

        // 右栏自己的状态栏：**当前也不放任何内容**（原「会话信息」标识已移除，
        // 该名称仍作下方 `SidebarGroup::new(RIGHT_SIDEBAR_LABEL)` 的段落标题）。
        // 同样**不画左边框**：列分界的竖线由分栏拖拽条统一画。
        let status_bar = StatusBar::new().h(STATUS_BAR_HEIGHT).w_full();

        v_flex()
            .h_full()
            .w_full()
            .overflow_hidden()
            .child(sidebar)
            .child(status_bar)
            .into_any_element()
    }
}
