//! 最左侧整列的装配:48px 图标栏 + 右侧侧边栏面板。

use std::path::PathBuf;

use gpui::{
    AnyElement, App, AsyncApp, Context, Entity, ExternalPaths, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, PathPromptOptions, SharedString, Styled as _, Window, div, px,
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Placement, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    h_flex,
    menu::{ContextMenuExt as _, PopupMenuItem},
};

use super::files::FilesState;
use super::{Sidebar, SidebarContent, SidebarView};
use crate::actions::{NewFolder, NewSession, OpenSettings};
use crate::assets::IconName;
use crate::components::{ActivityBar, ActivityBarItem, Sidebar as SidebarPanel};
use crate::dialog::name::open_name_dialog;
use crate::status_bar::empty_status_bar;

/// 整条左列:图标栏 + 侧边栏面板（面板自带底部状态栏）。
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
        // 工具栏只有「文件管理器」有（「会话」那条没有:它那一栏的动作在右键菜单里）。
        .toolbar(match active {
            SidebarView::Sessions => None,
            SidebarView::Files => Some(files_toolbar(sidebar.files.clone(), cx)),
        })
        .children(rows)
        // 侧边栏底部状态栏(与中间列那条同一个组件 / 同一个高度):空条。
        // 折叠时不摆:上面那个 `else` 早就 return 了。
        .status_bar(empty_status_bar());

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

/// 「文件管理器」顶部那一行工具栏（照 MobaXterm 的摆法：按钮在上、路径框在下）。
///
/// 九枚 **xsmall ghost 图标按钮**（靠左一字排开，中间两条细竖线分成三组）：
///
/// | 按钮 | 图标 | 动作 |
/// | --- | --- | --- |
/// | 上一目录 | `ArrowUp` | [`FilesState::navigate_up`] |
/// | 下载 | `Download` | [`FilesState::download_selected`]（**路径问用户**）|
/// | 上传 | `Upload` | `upload_via_dialog`（只问文件）|
/// | 刷新 | `Replace` | [`FilesState::refresh`] |
/// | 新建文件夹 | `FolderPlus` | 名字对话框 → [`FilesState::create_dir`] |
/// | 新建文件 | `File` | 名字对话框 → [`FilesState::create_file`] |
/// | 删除 | `Delete` | 确认对话框 → [`FilesState::remove_selected`] |
/// | 重命名 | `CaseSensitive` | 名字对话框（预填旧名）→ [`FilesState::rename_selected`] |
/// | 编辑 | `FileText` | [`FilesState::open_selected`] |
///
/// 判据每帧现取一次（[`FilesState::status`]）⇒ 跳目录 / 改选中项 / 断会话都会让相应的按钮
/// 变灰，不需要额外订阅。
///
/// ⚠️ 这几枚只做「最常用那一种」：「下载」只下**选中项**、**路径问用户**（与行右键菜单那
/// 两项同一条 [`FilesState::download_as`]）；「上传」只问**文件**（不问文件夹）；「删除」对
/// 目录是**整棵递归**（对话框里会说清）。其余都在**行右键菜单**里（上传文件夹 / 下载到
/// 下载目录 / 复制远端路径，见 `files::row_menu`）与**面板右键菜单**里（上传 / 刷新）。
fn files_toolbar(files: Entity<FilesState>, cx: &mut Context<Sidebar>) -> AnyElement {
    let status = files.read(cx).status();
    let has_selection = status.selected.is_some();
    let dir = status.selected_is_dir;
    let ready = status.ready;

    // ---- 上一目录 / 下载 / 上传 / 刷新 ----

    let up = {
        let files = files.clone();
        tool_button("files-up", IconName::ArrowUp, "上一目录", status.can_go_up, move |_, cx| {
            let _ = files.update(cx, |files, cx| files.navigate_up(cx));
        })
    };

    let download = {
        let files = files.clone();
        // 提示语跟着选中项的类型走（与行右键菜单那两项同一个措辞）。
        let tooltip = if dir { "下载文件夹到…" } else { "另存为…" };
        tool_button("files-download", IconName::Download, tooltip, has_selection, move |_, cx| {
            let _ = files.update(cx, |files, cx| files.download_selected(cx));
        })
    };

    let upload = {
        let files = files.clone();
        tool_button("files-upload", IconName::Upload, "上传文件…", ready, move |_, cx| {
            upload_via_dialog(&files, false, cx);
        })
    };

    let refresh = {
        let files = files.clone();
        tool_button("files-refresh", IconName::Replace, "刷新", ready, move |_, cx| {
            let _ = files.update(cx, |files, cx| files.refresh(cx));
        })
    };

    // ---- 新建文件夹 / 新建文件 ----

    let new_dir = {
        let files = files.clone();
        tool_button("files-new-dir", IconName::FolderPlus, "新建文件夹", ready, move |window, cx| {
            let files = files.clone();
            open_name_dialog(window, cx, "新建文件夹", "例如：logs", "创建", None, move |name, cx| {
                let _ = files.update(cx, |files, cx| files.create_dir(name, cx));
            });
        })
    };

    let new_file = {
        let files = files.clone();
        tool_button("files-new-file", IconName::File, "新建文件", ready, move |window, cx| {
            let files = files.clone();
            open_name_dialog(window, cx, "新建文件", "例如：notes.txt", "创建", None, move |name, cx| {
                let _ = files.update(cx, |files, cx| files.create_file(name, cx));
            });
        })
    };

    // ---- 删除 / 重命名 / 编辑 ----

    let remove = {
        let files = files.clone();
        let label = status.selected.clone().unwrap_or_default();
        tool_button("files-remove", IconName::Delete, "删除", has_selection, move |window, cx| {
            confirm_remove(window, cx, files.clone(), label.clone(), dir);
        })
    };

    let rename = {
        let files = files.clone();
        let label = status.selected.clone();
        tool_button("files-rename", IconName::CaseSensitive, "重命名", has_selection, move |window, cx| {
            let files = files.clone();
            // 预填旧名字：用户接着改，不用自己重打一遍。
            let initial = label.as_deref();
            open_name_dialog(window, cx, "重命名", "新名字", "重命名", initial, move |name, cx| {
                let _ = files.update(cx, |files, cx| files.rename_selected(name, cx));
            });
        })
    };

    let edit = {
        let files = files.clone();
        let tooltip = if dir { "展开 / 收起" } else { "编辑" };
        tool_button("files-edit", IconName::FileText, tooltip, has_selection, move |_, cx| {
            let _ = files.update(cx, |files, cx| files.open_selected(cx));
        })
    };

    h_flex()
        .w_full()
        .items_center()
        .gap_0p5()
        .children([up, download, upload, refresh])
        .child(separator(&cx.theme().border))
        .children([new_dir, new_file])
        .child(separator(&cx.theme().border))
        .children([remove, rename, edit])
        .into_any_element()
}

/// 工具栏上一枚图标按钮。
///
/// ⚠️ 尺寸取 `.xsmall()`：工具栏这一行与标题栏同高（28px），普通尺寸（≥20px 外还有 padding）
/// 会把整行撑开、把下面的列表挤小。
fn tool_button(
    id: &'static str,
    icon: IconName,
    tooltip: &'static str,
    enabled: bool,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> Button {
    Button::new(id)
        .ghost()
        .xsmall()
        .icon(icon)
        .tooltip(tooltip)
        .tooltip_placement(Placement::Top)
        .disabled(!enabled)
        .on_click(move |_, window, cx| on_click(window, cx))
}

/// 工具栏上两组按钮之间的细竖线。
fn separator(color: &Hsla) -> AnyElement {
    div()
        .flex_none()
        .w(px(1.))
        .h(px(14.))
        .mx_1()
        .bg(*color)
        .into_any_element()
}

/// 「删除」的确认对话框：危险操作，照主流文件管理器的做法先问一句。
///
/// ⚠️ 目录那一路是**整棵递归**删（远端那一侧见 `ssh::fs` 的 `remove`），所以提示语要把
/// 「连同里面的内容」说出来，别让用户以为是删个空壳。确认按钮用 `danger` 变体。
fn confirm_remove(
    window: &mut Window,
    cx: &mut App,
    files: Entity<FilesState>,
    label: SharedString,
    is_dir: bool,
) {
    let description = if is_dir {
        format!("确定要删除文件夹「{label}」吗？它里面的内容会一起删掉，且无法撤销。")
    } else {
        format!("确定要删除文件「{label}」吗？此操作无法撤销。")
    };

    window.open_dialog(cx, move |dialog, _window, _cx| {
        let files = files.clone();
        let footer = DialogFooter::new()
            .child(DialogClose::new().child(Button::new("remove-cancel").label("取消")))
            .child(
                DialogAction::new()
                    .child(Button::new("remove-confirm").label("删除").danger()),
            );

        dialog
            .title("删除")
            .w(px(380.))
            .footer(footer)
            .on_ok(move |_, _, cx| {
                let _ = files.update(cx, |files, cx| files.remove_selected(cx));
                true
            })
            .child(div().text_sm().child(description.clone()))
    });
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
