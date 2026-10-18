//! 侧边栏顶部的**面板标题栏**:当前视图的标题 + 右端「…」菜单。

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
    /// 本栏顶部的标题栏。
    pub(super) fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let active = self.active_view(cx);

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
            .child(view_menu(active, self.files_view_available(cx), cx.entity()))
            .into_any_element()
    }
}

/// 标题:当前视图的名字。
fn view_title(view: SidebarView) -> AnyElement {
    div()
        .min_w_0()
        .h_full()
        .flex()
        .items_center()
        .child(ellipsis_label(view.label().into()))
        .into_any_element()
}

/// 右端的「…」视图菜单。
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
