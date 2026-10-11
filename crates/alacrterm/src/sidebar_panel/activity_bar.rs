//! 最左侧整列的装配:48px 的图标栏(每个视图一枚 + 底部「设置」) + 右侧的侧边栏面板
//! (当前视图的面板标题栏 + 内容)。
//!
//! 两块都是 `components/` 里的组件(图标栏 [`ActivityBar`]、面板 [`SidebarPanel`]);
//! 这里把本应用的语义填进去:每个视图一枚图标([`SidebarView`])、面板里摆当前视图的行
//! ([`SidebarContent`])。
//!
//! 整列常驻(分栏组的第 0 个面板);点击语义见 [`Sidebar::toggle_view`]:点一枚 = 显示这个
//! 视图,再点当前正在显示的那枚 = 折叠侧边栏。高亮的即当前显示的视图,折叠时一枚都不亮。

use gpui::{AnyElement, Context, IntoElement, ParentElement as _, SharedString, Styled as _, div};
use gpui_kit::component::menu::ContextMenuExt as _;

use super::{Sidebar, SidebarContent, SidebarView};
use crate::actions::{NewFolder, NewSession, OpenSettings};
use crate::assets::IconName;
use crate::components::{ActivityBar, ActivityBarItem, Sidebar as SidebarPanel};

/// 本应用的整条左列:图标栏 + 侧边栏面板(折叠时不摆面板,只剩图标栏)。
pub(super) fn left_column(sidebar: &mut Sidebar, cx: &mut Context<Sidebar>) -> AnyElement {
    // 折叠时一枚都不亮:亮着的是当前显示的视图。
    let active = if sidebar.visible() {
        sidebar.active_view()
    } else {
        None
    };
    let files_enabled = sidebar.files_enabled;

    let bar = SidebarView::ALL
        .iter()
        .copied()
        // 不可用的视图(本地会话下的「文件管理器」)连图标一起不摆。
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
        // 底部「设置」入口(派发全局 action)。
        .footer_item(ActivityBarItem::new(
            "activity-settings",
            IconName::Settings,
            "设置",
            |_, window, cx| window.dispatch_action(Box::new(OpenSettings), cx),
        ));

    let Some(active) = active else {
        return bar.into_any_element();
    };

    // 内容:一组行(会话视图一条记录一行;文件管理器一行一项)。
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
        // 面板标题栏。
        .header(sidebar.render_header(cx))
        .children(rows);

    // 「会话」视图:整块内容的右键菜单 = 顶层「新建会话 / 新建文件夹」
    // (条目自己的右键菜单见 [`super::sessions`])。
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
