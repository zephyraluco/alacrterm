//! 最左侧整列的装配:48px 图标栏 + 右侧侧边栏面板。

use gpui::{AnyElement, Context, IntoElement, ParentElement as _, SharedString, Styled as _, div};
use gpui_kit::component::menu::ContextMenuExt as _;

use super::{Sidebar, SidebarContent, SidebarView};
use crate::actions::{NewFolder, NewSession, OpenSettings};
use crate::assets::IconName;
use crate::components::{ActivityBar, ActivityBarItem, Sidebar as SidebarPanel};

/// 整条左列:图标栏 + 侧边栏面板。
pub(super) fn left_column(sidebar: &mut Sidebar, cx: &mut Context<Sidebar>) -> AnyElement {
    let active = if sidebar.visible() {
        sidebar.active_view()
    } else {
        None
    };
    let files_enabled = sidebar.files_enabled;

    let bar = SidebarView::ALL
        .iter()
        .copied()
        .filter(|view| view.visible_with(files_enabled))
        .fold(ActivityBar::new("activity-bar"), |bar, view| {
            let state = cx.weak_entity();
            bar.item(
                ActivityBarItem::new(view.id(), view.icon(), view.tooltip(), move |_, _, cx| {
                    let _ = state.update(cx, |sidebar, cx| sidebar.toggle_view(view, cx));
                })
                .active(active == Some(view)),
            )
        })
        .footer_item(ActivityBarItem::new(
            "activity-settings",
            IconName::Settings,
            "设置",
            |_, window, cx| window.dispatch_action(Box::new(OpenSettings), cx),
        ));

    let Some(active) = active else {
        return bar.into_any_element();
    };

    let rows: Vec<SidebarContent> = match active {
        SidebarView::Sessions => sidebar
            .sessions
            .update(cx, |sessions, cx| sessions.sidebar_items(cx))
            .into_iter()
            .map(SidebarContent::Sessions)
            .collect(),
        SidebarView::Files => sidebar
            .files
            .update(cx, |files, cx| files.sidebar_items(cx))
            .into_iter()
            .map(SidebarContent::Files)
            .collect(),
    };

    let panel = SidebarPanel::new(SharedString::from(format!("sidebar-{}", active.id())))
        .header(sidebar.render_header(cx))
        .children(rows);

    let panel = match active {
        SidebarView::Sessions => div()
            .size_full()
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
            .child(panel)
            .into_any_element(),
        SidebarView::Files => panel.into_any_element(),
    };

    bar.sidebar(panel).into_any_element()
}
