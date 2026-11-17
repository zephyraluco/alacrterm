//! File browser business panel, toolbar, menus and drop handling.
use gpui::{ListAlignment, ListState, list, prelude::FluentBuilder as _};
use gpui_kit::base::Scrollbar;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    AnyElement, App, AsyncApp, Context, Entity, ExternalPaths, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, PathPromptOptions, SharedString, Styled as _, Window, div, px,
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Placement, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{ContextMenuExt as _, PopupMenuItem},
    v_flex,
};

use super::files::FilesState;
use super::header::panel_header;
use crate::assets::IconName;
use crate::components::{PanelId, PanelInfo, SidebarPanel};
use crate::dialog::name::open_name_dialog;
use gpui::{Render, Subscription};

pub(crate) const FILES_PANEL: PanelId = PanelId("files");

pub(crate) struct FilesPanel {
    files: Entity<FilesState>,
    enabled: bool,
    _subscription: Subscription,
    list_state: ListState,
}
impl FilesPanel {
    pub(crate) fn new(files: Entity<FilesState>, cx: &mut Context<Self>) -> Self {
        Self {
            files: files.clone(),
            enabled: false,
            list_state: ListState::new(0, ListAlignment::Top, px(400.)),
            _subscription: cx.observe(&files, |_, _, cx| cx.notify()),
        }
    }
    pub(crate) fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.enabled != enabled {
            self.enabled = enabled;
            cx.notify();
        }
    }
}
impl SidebarPanel for FilesPanel {
    const INFO: PanelInfo = PanelInfo {
        id: FILES_PANEL,
        title: "文件管理器",
        icon: IconName::Folder,
        tooltip: "当前终端目录下的内容",
        default_width: px(220.),
    };
    fn available(&self, cx: &App) -> bool {
        self.enabled || self.files.read(cx).has_transfers(cx)
    }
}
impl Render for FilesPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.files.update(cx, |files, cx| files.sidebar_items(cx));
        let count = rows.len();
        if self.list_state.item_count() != count {
            self.list_state.reset(count);
        }
        let rows = Rc::new(rows);
        let state = self.list_state.clone();
        let content = div()
            .id("files-scroll")
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                v_flex().size_full().px_3().child(
                    list(state.clone(), move |index, _, cx| {
                        div()
                            .id(index)
                            .when(index == 0, |row| row.pt_3())
                            .when(index + 1 == count, |row| row.pb_3())
                            .when_some(rows.get(index).cloned(), |row, item| {
                                row.child(item.render(cx))
                            })
                            .into_any_element()
                    })
                    .size_full(),
                ),
            )
            .child(
                div().absolute().inset_0().child(
                    Scrollbar::vertical(&state)
                        .id("files-scrollbar")
                        .viewport_from_layout(),
                ),
            );

        let files = self.files.read(cx);
        let ready = files.status().ready;
        let path_input = files.render_path_input(self.files.clone(), cx);
        let toolbar = v_flex()
            .w_full()
            .flex_none()
            .gap_2()
            .children(ready.then(|| files_toolbar(self.files.clone(), cx)))
            .children(path_input)
            .into_any_element();
        let panel = v_flex()
            .id("sidebar-files")
            .size_full()
            .overflow_hidden()
            .bg(cx.theme().tokens.sidebar)
            .text_color(cx.theme().sidebar_foreground)
            .child(
                v_flex()
                    .pt_3()
                    .px_3()
                    .child(panel_header(Self::INFO.title, cx)),
            )
            .child(v_flex().px_3().pb_2().child(toolbar))
            .child(content)
            .into_any_element();
        div()
            .size_full()
            .key_context(crate::keymap::SIDEBAR_CONTEXT)
            .child(files_panel(panel, self.files.clone()))
    }
}

fn files_toolbar(files: Entity<FilesState>, cx: &mut Context<FilesPanel>) -> AnyElement {
    let status = files.read(cx).status();
    let has_selection = status.selected.is_some();
    let dir = status.selected_is_dir;
    let ready = status.ready;

    // ---- 上一目录 / 下载 / 上传 / 刷新 ----

    let up = {
        let files = files.clone();
        tool_button(
            "files-up",
            IconName::ArrowUp,
            "上一目录",
            status.can_go_up,
            move |_, cx| {
                files.update(cx, |files, cx| files.navigate_up(cx));
            },
        )
    };

    let download = {
        let files = files.clone();
        // 提示语跟着选中项的类型走（与行右键菜单那两项同一个措辞）。
        let tooltip = if dir {
            "下载文件夹到…"
        } else {
            "另存为…"
        };
        tool_button(
            "files-download",
            IconName::Download,
            tooltip,
            has_selection,
            move |_, cx| {
                files.update(cx, |files, cx| files.download_selected(cx));
            },
        )
    };

    let upload = {
        let files = files.clone();
        tool_button(
            "files-upload",
            IconName::Upload,
            "上传文件…",
            ready,
            move |_, cx| {
                upload_via_dialog(&files, false, cx);
            },
        )
    };

    let refresh = {
        let files = files.clone();
        tool_button(
            "files-refresh",
            IconName::Refresh,
            "刷新",
            ready,
            move |_, cx| {
                files.update(cx, |files, cx| files.refresh(cx));
            },
        )
    };

    // ---- 新建文件夹 / 新建文件 ----

    let new_dir = {
        let files = files.clone();
        tool_button(
            "files-new-dir",
            IconName::FolderPlus,
            "新建文件夹",
            ready,
            move |window, cx| {
                let files = files.clone();
                open_name_dialog(
                    window,
                    cx,
                    "新建文件夹",
                    "例如：logs",
                    "创建",
                    None,
                    move |name, cx| {
                        files.update(cx, |files, cx| files.create_dir(name, cx));
                    },
                );
            },
        )
    };

    let new_file = {
        let files = files.clone();
        tool_button(
            "files-new-file",
            IconName::NewFile,
            "新建文件",
            ready,
            move |window, cx| {
                let files = files.clone();
                open_name_dialog(
                    window,
                    cx,
                    "新建文件",
                    "例如：notes.txt",
                    "创建",
                    None,
                    move |name, cx| {
                        files.update(cx, |files, cx| files.create_file(name, cx));
                    },
                );
            },
        )
    };

    // ---- 重命名 ----

    let rename = {
        let files = files.clone();
        let label = status.selected.clone();
        tool_button(
            "files-rename",
            IconName::CaseSensitive,
            "重命名",
            has_selection,
            move |window, cx| {
                let files = files.clone();
                // 预填旧名字：用户接着改，不用自己重打一遍。
                let initial = label.as_deref();
                open_name_dialog(
                    window,
                    cx,
                    "重命名",
                    "新名字",
                    "重命名",
                    initial,
                    move |name, cx| {
                        files.update(cx, |files, cx| files.rename_selected(name, cx));
                    },
                );
            },
        )
    };

    h_flex()
        .w_full()
        .items_center()
        .gap_0p5()
        .children([up, download, upload, refresh])
        .child(separator(&cx.theme().border))
        .children([new_dir, new_file])
        .child(separator(&cx.theme().border))
        .child(rename)
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
        .icon(crate::assets::ui_icon(icon))
        .tooltip(tooltip)
        .tooltip_placement(Placement::Top)
        .disabled(!enabled)
        .on_click(move |_, window, cx| on_click(window, cx))
}

/// 工具栏和状态栏中划分按钮区域的细竖线。
fn separator(color: &Hsla) -> AnyElement {
    div()
        .flex_none()
        .w(px(1.))
        .h(px(14.))
        .mx_1()
        .bg(*color)
        .into_any_element()
}

/// 「文件管理器」面板:挂上传 / 刷新的右键菜单 + **拖入即上传**。
///
/// ⚠️ 拖放要能命中**整块面板**才符合直觉(用户不会瞄着某一行拖)。平台层把外部拖拽翻译成
/// 内部的 `ExternalPaths` 拖拽(见 `gpui_pre::window` 的 `FileDropEvent`),所以只要
/// `.on_drop::<ExternalPaths>`;落到哪个目录由 `FilesState::upload_target_dir` 决定
/// (选中项 ⇒ 它所在的目录,没选中 ⇒ 当前根目录)。
fn files_panel(panel: AnyElement, files: Entity<FilesState>) -> AnyElement {
    let drop_target = files.clone();
    let upload_target = files.clone();
    let folder_target = files.clone();
    let refresh_target = files;
    div()
        .size_full()
        .on_drop::<ExternalPaths>(move |paths: &ExternalPaths, _, cx| {
            let locals: Vec<PathBuf> = paths.paths().to_vec();
            drop_target.update(cx, |files, cx| files.start_upload(locals, cx));
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
                    .icon(crate::assets::ui_icon(IconName::Upload))
                    .on_click(move |_, _, cx| {
                        upload_via_dialog(&upload, false, cx);
                    }),
            )
            .item(
                PopupMenuItem::new("上传文件夹…")
                    .icon(crate::assets::ui_icon(IconName::Upload))
                    .on_click(move |_, _, cx| {
                        upload_via_dialog(&folder, true, cx);
                    }),
            )
            .separator()
            .item(
                PopupMenuItem::new("刷新")
                    .icon(crate::assets::ui_icon(IconName::Refresh))
                    .on_click(move |_, _, cx| {
                        refresh.update(cx, |files, cx| files.refresh(cx));
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
        files.update(cx, |files, cx| files.start_upload(paths, cx));
    })
    .detach();
}
