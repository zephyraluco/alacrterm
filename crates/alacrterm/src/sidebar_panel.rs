//! 左侧容器：活动栏 + 侧边栏 + 侧边栏状态栏。
//!
//! 三部分同属「左侧区域」，但装配方式不同（见 [`crate::AppRoot::render`]）：
//! - **活动栏**（[`AppRoot::render_activity_bar`]）：固定 [`ACTIVITY_BAR_WIDTH`] 宽、
//!   直到底部，常驻显示且不参与分栏拖拽——侧边栏折叠后仍靠它恢复。
//!   上半部分是视图切换图标（终端会话 / 关于），弹性占位后设置图标固定在底部。
//! - **侧边栏**（[`AppRoot::render_sidebar_container`] 上半部分）：宽度由根视图的
//!   分栏面板（可拖拽分隔条）决定，故自身只需 `w_full`。内容为
//!   「[`SidebarGroup`] 段落标题 + 段内可折叠菜单」两层结构：段落标题给出分区
//!   （会话 / 关于 / 版本信息），段内菜单项是官方「嵌套菜单」形态，
//!   点击表头行即展开 / 收起下方的会话列表。
//! - **状态栏**（同上下半部分）：贴在侧边栏底部，宽度随侧边栏一起变化，
//!   与右侧终端容器的状态栏同高对齐，仅以右缘竖线分隔。

use gpui::{
    AnyElement, Context, IntoElement, ParentElement as _, Pixels, Styled as _, div, px,
};
use gpui_kit::component::{
    ActiveTheme as _, Icon, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    sidebar::{Sidebar, SidebarGroup, SidebarMenu, SidebarMenuItem},
    status_bar::StatusBar,
    v_flex,
};

use crate::actions::{CloseSession, NewTerminal};
use crate::AppRoot;
use crate::assets::IconName;

/// 活动栏宽度（固定，不参与分栏拖拽）。
const ACTIVITY_BAR_WIDTH: Pixels = px(44.);

/// 侧边栏默认宽度（分栏面板首次布局时的初始宽度）。
pub(crate) const SIDEBAR_DEFAULT_WIDTH: Pixels = px(220.);
/// 侧边栏拖拽时的最小宽度（须大于组件内部的 `PANEL_MIN_SIZE` = 100px）。
pub(crate) const SIDEBAR_MIN_WIDTH: Pixels = px(150.);
/// 侧边栏拖拽时的最大宽度。
pub(crate) const SIDEBAR_MAX_WIDTH: Pixels = px(460.);

/// 侧边栏视图（对应活动栏图标）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SidebarView {
    /// 终端会话列表。
    Sessions,
    /// 关于。
    About,
}

impl AppRoot {
    /// 活动栏图标点击：切换视图；再次点击当前视图图标则隐藏 / 显示侧边栏。
    pub(crate) fn set_sidebar_view(&mut self, view: SidebarView, cx: &mut Context<Self>) {
        if self.sidebar_view == view {
            self.sidebar_visible = !self.sidebar_visible;
        } else {
            self.sidebar_view = view;
            self.sidebar_visible = true;
        }
        cx.notify();
    }

    /// 活动栏：侧边栏左侧的图标列（图标按钮 + 底部设置入口）。
    ///
    /// 返回 [`AnyElement`] 而非 `impl IntoElement`：本 crate 是 edition 2024，
    /// `impl Trait` 会捕获 `&mut Context` 的生命周期，导致同一渲染树里
    /// 连续调用多个 `&mut cx` 的渲染方法时借用冲突；装箱可彻底规避。
    pub(crate) fn render_activity_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .w(ACTIVITY_BAR_WIDTH)
            .h_full()
            .flex_shrink_0()
            .items_center()
            .py_2()
            .gap_1()
            .bg(cx.theme().tokens.sidebar)
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .child(
                Button::new("view-sessions")
                    .ghost()
                    .icon(IconName::SquareTerminal)
                    .selected(self.sidebar_visible && self.sidebar_view == SidebarView::Sessions)
                    .tooltip("终端会话")
                    .on_click(
                        cx.listener(|this, _, _, cx| this.set_sidebar_view(SidebarView::Sessions, cx)),
                    ),
            )
            .child(
                Button::new("view-about")
                    .ghost()
                    .icon(IconName::Info)
                    .selected(self.sidebar_visible && self.sidebar_view == SidebarView::About)
                    .tooltip("关于")
                    .on_click(
                        cx.listener(|this, _, _, cx| this.set_sidebar_view(SidebarView::About, cx)),
                    ),
            )
            // 弹性占位：把设置图标推到活动栏最下方。
            .child(div().flex_1())
            .child(
                Button::new("view-settings")
                    .ghost()
                    .icon(IconName::Settings)
                    .tooltip("设置")
                    // 打开独立的设置窗口（重复点击只激活已有窗口）。
                    .on_click(cx.listener(|this, _, _, cx| this.open_settings_window(cx))),
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
                .on_click(move |_, _, cx| {
                    let _ = this.update(cx, |this, cx| this.set_active_tab(ix, cx));
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
        // flex_1 + min_h_0：与下方状态栏同处一列，需能收缩，避免把状态栏挤出容器。
        let sidebar = Sidebar::new(sidebar_id)
            .w_full()
            .child(content)
            .flex_1()
            .min_h_0();

        // 底部状态栏：宽度跟随侧边栏（w_full），右缘竖线区分右侧终端段。
        let status_bar = StatusBar::new()
            .left(
                h_flex()
                    .items_center()
                    .gap_1()
                    .child(Icon::new(IconName::SquareTerminal).small())
                    .child(self.shell_name.clone()),
            )
            .w_full()
            .border_r_1();

        // 整个侧边栏区域（会话列表 + 底部状态栏）。
        // 空白区域**不挂**右键菜单，只有会话条目有自己的右键菜单。
        v_flex()
            .h_full()
            .w_full()
            .overflow_hidden()
            .child(sidebar)
            .child(status_bar)
            .into_any_element()
    }
}
