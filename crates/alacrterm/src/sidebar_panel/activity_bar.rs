//! 最左侧整列的装配:48px 图标栏 + 右侧侧边栏面板。

use std::path::PathBuf;

use gpui::{
    AnyElement, App, AsyncApp, Context, Entity, ExternalPaths, InteractiveElement as _,
    IntoElement, ParentElement as _, PathPromptOptions, SharedString, Styled as _, div,
};
use gpui_kit::component::{
    ActiveTheme as _,
    menu::{ContextMenuExt as _, PopupMenuItem},
};

use super::files::FilesState;
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
        SidebarView::Files => files_panel(panel, sidebar.files.clone()),
    };

    bar.sidebar(panel).into_any_element()
}

/// 「文件管理器」面板:挂上传 / 刷新的右键菜单 + **拖入即上传**。
///
/// ⚠️ 拖放要能命中**整块面板**才符合直觉(用户不会瞄着某一行拖)。平台层把外部拖拽翻译成
/// 内部的 `ExternalPaths` 拖拽(见 `gpui_pre::window` 的 `FileDropEvent`),所以只要
/// `.on_drop::<ExternalPaths>`;落到哪个目录由 `FilesState::upload_target_dir` 决定
/// (选中项 ⇒ 它所在的目录,没选中 ⇒ 当前根目录)。
fn files_panel(panel: SidebarPanel<SidebarContent>, files: Entity<FilesState>) -> AnyElement {
    let drop_target = files.clone();
    let upload_target = files.clone();
    let folder_target = files.clone();
    let refresh_target = files;
    div()
        .size_full()
        .on_drop::<ExternalPaths>(move |paths: &ExternalPaths, _, cx| {
            let locals: Vec<PathBuf> = paths.paths().to_vec();
            let _ = drop_target.update(cx, |files, cx| files.start_upload(locals, cx));
        })
        // 拖拽悬停时把整块面板点亮 ⇒ 用户知道这里能放(样式回调每帧按「当前有没有在拖」问一次,
        // 不需要自己存状态)。
        .drag_over::<ExternalPaths>(|style, _, _, cx| {
            style
                .bg(cx.theme().tokens.accent)
                .border_1()
                .border_color(cx.theme().primary)
        })
        .context_menu(move |menu, _window, _cx| {
            // 这个回调是 `Fn`(每帧都可能被调用),所以每次进来都要现取一份实体句柄。
            let upload = upload_target.clone();
            let folder = folder_target.clone();
            let refresh = refresh_target.clone();
            menu.item(
                PopupMenuItem::new("上传文件…")
                    .icon(IconName::Upload)
                    .on_click(move |_, _, cx| {
                        upload_via_dialog(&upload, false, cx);
                    }),
            )
            .item(
                PopupMenuItem::new("上传文件夹…")
                    .icon(IconName::Upload)
                    .on_click(move |_, _, cx| {
                        upload_via_dialog(&folder, true, cx);
                    }),
            )
            .separator()
            .item(
                PopupMenuItem::new("刷新")
                    .icon(IconName::Replace)
                    .on_click(move |_, _, cx| {
                        let _ = refresh.update(cx, |files, cx| files.refresh(cx));
                    }),
            )
        })
        .child(panel)
        .into_any_element()
}

/// 「上传文件…」/「上传文件夹…」:问系统要本地路径,再交给文件管理器去传。
fn upload_via_dialog(files: &Entity<FilesState>, directories: bool, cx: &mut App) {
    let prompt: SharedString = if directories {
        "选择要上传的文件夹".into()
    } else {
        "选择要上传的文件".into()
    };
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: !directories,
        directories,
        multiple: true,
        prompt: Some(prompt),
    });
    let files = files.clone();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let Ok(Ok(Some(paths))) = picked.await else {
            return;
        };
        let _ = files.update(cx, |files, cx| files.start_upload(paths, cx));
    })
    .detach();
}
