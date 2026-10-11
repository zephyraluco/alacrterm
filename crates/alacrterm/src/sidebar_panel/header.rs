//! 侧边栏顶部的**面板标题栏**:当前视图的标题 + 右端「…」菜单(照 VS Code 的侧边栏标题栏)。
//!
//! 面板里不再排标签:显示哪个视图由**活动栏**点选(见 [`super::activity_bar`]),这一行只摆
//! 当前那个的标题。「…」菜单把**可用的视图**都列出来、勾选当前那个,点一下就切过去
//! ([`Sidebar::open_view`]) —— 与活动栏的图标等价,只是不用把鼠标挪回最左列。

use gpui::{
    Anchor, AnyElement, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Styled as _, div,
};
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{DropdownMenu as _, PopupMenuItem},
};

use super::shared::ellipsis_label;
use super::{PANEL_HEADER_HEIGHT, Sidebar, SidebarView};
use crate::assets::IconName;

impl Sidebar {
    /// 本栏顶部的标题栏(挂在 `Sidebar` 的 `header` 上 ⇒ 固定顶部、不随内容滚动)。
    ///
    /// 返回 [`AnyElement`] 而非 `impl IntoElement`:edition 2024 下 `impl Trait` 会捕获
    /// `&mut Context` 的生命周期,同一渲染树里连续调用多个渲染方法会借用冲突。
    pub(super) fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let active = self.active_view();

        h_flex()
            .id("sidebar-header")
            .h(PANEL_HEADER_HEIGHT)
            .w_full()
            .items_center()
            .justify_between()
            .gap_2()
            .overflow_hidden()
            .rounded(cx.theme().radius)
            .text_xs()
            .text_color(cx.theme().sidebar_foreground)
            .children(active.map(view_title))
            .child(view_menu(active, self.files_enabled, cx.entity()))
            .into_any_element()
    }
}

/// 标题:当前视图的名字(没有可用视图时整块不摆)。
fn view_title(view: SidebarView) -> AnyElement {
    div()
        .min_w_0()
        .h_full()
        .flex()
        .items_center()
        .child(ellipsis_label(view.label().into()))
        .into_any_element()
}

/// 右端的「…」菜单:每个可用视图一项、勾选当前那个,点一下就切过去([`Sidebar::open_view`])。
fn view_menu(
    active: Option<SidebarView>,
    files_enabled: bool,
    sidebar: Entity<Sidebar>,
) -> AnyElement {
    Button::new("sidebar-menu")
        .ghost()
        .xsmall()
        .icon(IconName::Ellipsis)
        .tooltip("视图")
        // 菜单右对齐弹出(VS Code 的「…」菜单也是贴在右端往左下展开)。
        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _window, _cx| {
            SidebarView::ALL
                .iter()
                .copied()
                .filter(|view| view.visible_with(files_enabled))
                .fold(menu, |menu, view| {
                    let sidebar = sidebar.clone();
                    menu.item(
                        PopupMenuItem::new(view.label())
                            .checked(active == Some(view))
                            .on_click(move |_, _window, cx| {
                                let _ = sidebar.update(cx, |sidebar, cx| sidebar.open_view(view, cx));
                            }),
                    )
                })
        })
        .into_any_element()
}
