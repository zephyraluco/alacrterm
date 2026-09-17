//! 侧边栏（左侧 / 右侧）与它们的折叠开关。
//!
//! 两侧边栏 + 开关 + 活动栏图标分处两个模块，装配见 [`crate::AppRoot::render`]：
//! - **左侧边栏**（[`AppRoot::render_sidebar_container`]）：宽度由分栏面板（可拖拽分隔条）
//!   决定，故自身只需 `w_full`。内容为「[`SidebarGroup`] 段落标题 + 段内可折叠菜单」
//!   两层结构：段落标题给出分区（会话 / 关于 / 版本信息），段内菜单项是官方
//!   「嵌套菜单」形态，点击表头行即展开 / 收起下方的会话列表。
//! - **右侧边栏**（[`AppRoot::render_right_sidebar_container`]）：终端右侧的面板，
//!   当前展示当前会话的只读信息（名称 / 连接 / 进程 / 状态）；用 `Side::Right` 构造，
//!   与左侧对称。
//! - **两枚折叠开关**（[`AppRoot::sidebar_toggle_button`] /
//!   [`AppRoot::right_sidebar_toggle_button`]）：分别渲染在**各自那一条状态栏**里
//!   （左栏在自身状态栏的最左端、右栏在最右端），图标随各自的折叠状态变化。
//!   左栏开关旁边还会渲染活动栏图标（[`AppRoot::render_activity_icons`]：终端会话 / 关于），
//!   它们只在左侧边栏可见时显示，且**只切视图、不会折叠侧边栏**。
//!   某一侧折叠后它那一整块（含自己的状态栏与开关）不渲染，改由中间那条公共状态栏
//!   在**同一侧**补一枚「展开」按钮（左端 / 右端，见 [`crate::status_bar`]）。
//!
//! 两侧边栏用**两组嵌套的分栏面板**装配（内层 `main-split`、外层 `right-split`），
//! 因为面板宽度按下标存在 `ResizableState` 里：三个面板挤在同一组时，
//! 任一侧折叠都会让另一侧的下标漂移、拖出来的宽度丢失。
//! 「设置」入口不在本模块，而在标题栏右侧的文字按钮上（见 [`crate::AppRoot::render`]）。

use gpui::{AnyElement, Context, IntoElement, ParentElement as _, Pixels, SharedString, Styled as _, px};
use gpui_kit::component::{
    Icon, Selectable as _, Side, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    sidebar::{Sidebar, SidebarGroup, SidebarMenu, SidebarMenuItem},
    status_bar::StatusBar,
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

/// 右侧边栏的名称：既作面板里的段落标题，也作它在状态栏右段里的标识文字。
pub(crate) const RIGHT_SIDEBAR_LABEL: &str = "会话信息";

/// 侧边栏视图（对应活动栏图标）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SidebarView {
    /// 终端会话列表。
    Sessions,
    /// 关于。
    About,
}

impl AppRoot {
    /// 活动栏图标点击：切换左栏视图。
    ///
    /// **只切视图，不改可见性**——折叠 / 展开只由状态栏里的折叠按钮负责，
    /// 与右侧边栏一致（那边除折叠按钮外没有任何按钮会改可见性）。
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

    /// 设置左侧边栏是否显示（只由状态栏里的折叠按钮调用）。
    pub(crate) fn set_sidebar_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.sidebar_visible == visible {
            return;
        }
        self.sidebar_visible = visible;
        cx.notify();
    }

    /// 设置右侧边栏是否显示（状态栏右端的折叠开关调用）。
    pub(crate) fn set_right_sidebar_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.right_sidebar_visible == visible {
            return;
        }
        self.right_sidebar_visible = visible;
        cx.notify();
    }

    /// 右侧边栏折叠 / 展开开关（挂在**右栏自己那条状态栏的最右端**）。
    ///
    /// 与左侧开关一样常驻：状态栏不随右侧边栏折叠消失，所以它是唯一的恢复入口。
    /// 图标随状态变化（`PanelRightClose` ↔ `PanelRightOpen`）。
    pub(crate) fn right_sidebar_toggle_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let expanded = self.right_sidebar_visible;
        Button::new("right-sidebar-toggle")
            .ghost()
            .xsmall()
            // 与左侧开关一致：图标按钮默认 20px 高，显式压到状态栏行高（16px）。
            .h(px(16.))
            .icon(if expanded {
                IconName::PanelRightClose
            } else {
                IconName::PanelRightOpen
            })
            .tooltip(if expanded {
                "折叠右侧边栏"
            } else {
                "展开右侧边栏"
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_right_sidebar_visible(!expanded, cx)
            }))
            .into_any_element()
    }

    /// 侧边栏折叠 / 展开开关（挂在**左栏自己那条状态栏的最左端**，即窗口左下角）。
    ///
    /// 图标与提示随状态变化：侧边栏可见时是「折叠」，隐藏时是「展开」。
    /// 这是**唯一**能改变左侧边栏可见性的入口（活动栏图标只切视图，不会折叠它），
    /// 与右侧边栏一致：那边也只有它自己那枚开关。另：折叠后这一整块不渲染，
    /// 「展开」按钮改由中间那条公共状态栏的最左端提供，所以不存在「折叠完找不回来」。
    pub(crate) fn sidebar_toggle_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let expanded = self.sidebar_visible;
        Button::new("sidebar-toggle")
            .ghost()
            .xsmall()
            // 图标按钮默认 20px 高，会把状态栏撑高（`text_xs` 行高≈16px），
            // 这里显式压到 16px。
            .h(px(16.))
            .icon(if expanded {
                IconName::PanelLeftClose
            } else {
                IconName::PanelLeftOpen
            })
            .tooltip(if expanded {
                "折叠侧边栏"
            } else {
                "展开侧边栏"
            })
            .on_click(cx.listener(move |this, _, _, cx| this.set_sidebar_visible(!expanded, cx)))
            .into_any_element()
    }

    /// 活动栏图标（终端会话 / 关于）。
    ///
    /// 原先是侧边栏左侧一条 44px 宽的竖栏，现已**整体迁移到左栏状态栏里**
    /// （与折叠按钮同处一条，见 [`AppRoot::render_sidebar_container`]），横向排开：
    /// 侧边栏可见时才由那条状态栏渲染，折叠后不渲染（没有可切换的视图，留着只是占地方）。
    /// 点击**只切换视图**（`set_sidebar_view`）：不会折叠 / 展开侧边栏。
    /// 设置入口不在这里——它是标题栏右侧的「设置」文字按钮（见 `AppRoot::render`）。
    ///
    /// 返回 [`AnyElement`] 而非 `impl IntoElement`：本 crate 是 edition 2024，
    /// `impl Trait` 会捕获 `&mut Context` 的生命周期，导致同一渲染树里
    /// 连续调用多个 `&mut cx` 的渲染方法时借用冲突；装箱可彻底规避。
    pub(crate) fn render_activity_icons(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .items_center()
            .gap_1()
            // 状态栏里的按钮统一压到 16px 高（≈`text_xs` 行高），免得把状态栏撑高。
            .child(
                Button::new("view-sessions")
                    .ghost()
                    .xsmall()
                    .h(px(16.))
                    .icon(IconName::SquareTerminal)
                    .selected(self.sidebar_view == SidebarView::Sessions)
                    .tooltip("终端会话")
                    .on_click(
                        cx.listener(|this, _, _, cx| this.set_sidebar_view(SidebarView::Sessions, cx)),
                    ),
            )
            .child(
                Button::new("view-about")
                    .ghost()
                    .xsmall()
                    .h(px(16.))
                    .icon(IconName::Info)
                    .selected(self.sidebar_view == SidebarView::About)
                    .tooltip("关于")
                    .on_click(
                        cx.listener(|this, _, _, cx| this.set_sidebar_view(SidebarView::About, cx)),
                    ),
            )
            .into_any_element()
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

        // 侧边栏内容：随活动栏选中的视图切换。
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
            .child(content)
            .flex_1()
            .min_h_0();

        // 左栏自己的状态栏：折叠按钮 + 视图图标（终端会话 / 关于）。
        // 它与侧边栏同处一个列容器，宽度自然随侧边栏（拖分隔条时实时跟随）；
        // **不画右边框**：列分界的那条竖线统一由分栏拖拽条来画
        // （它 `h_full` 贯穿整列，也盖住这一行；见 `crate::change_theme` 的说明）。
        // 高度用 `STATUS_BAR_HEIGHT`：这条里没有文字，自然高度比含文字的那两条矮，
        // 不统一就会出现「左栏那条短一截」的错位。
        let status_bar = StatusBar::new()
            .left(self.sidebar_toggle_button(cx))
            .left(self.render_activity_icons(cx))
            .h(STATUS_BAR_HEIGHT)
            .w_full();

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
            .child(content)
            .flex_1()
            .min_h_0();

        // 右栏自己的状态栏：标识（图标 + 名称）在左、折叠按钮在右端——
        // 与左栏那一条镜像对称（那边是「按钮在左端 + 图标在其右」）。
        // 同样**不画左边框**：列分界的竖线由分栏拖拽条统一画。
        let status_bar = StatusBar::new()
            .left(
                h_flex()
                    .items_center()
                    .gap_1()
                    .flex_shrink_0()
                    .child(Icon::new(IconName::Info).small())
                    .child(RIGHT_SIDEBAR_LABEL),
            )
            .right(self.right_sidebar_toggle_button(cx))
            .h(STATUS_BAR_HEIGHT)
            .w_full();

        v_flex()
            .h_full()
            .w_full()
            .overflow_hidden()
            .child(sidebar)
            .child(status_bar)
            .into_any_element()
    }
}
