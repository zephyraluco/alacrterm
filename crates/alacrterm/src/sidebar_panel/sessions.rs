//! 「会话」视图:文件夹形式的会话记录树,入口 [`SessionsState`]。
//! 行骨架与摊平走 [`super::shared`],记录持久化到 `config/sessions.json`。

use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, Context, CursorStyle, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity,
    Window, div, px,
};
use gpui_kit::component::{ActiveTheme as _, menu::ContextMenuExt as _};

use super::empty_state;
use super::shared::{DragPreview, RowCache, row_content, row_shell};
use crate::actions::{EditSession, MoveEntry, NewFolder, NewSession, OpenSession, RemoveEntry};
use crate::assets::IconName;
use crate::terminal_panel::SessionRequest;

use session::{SessionEntry, SessionTree};
pub(crate) use session::{SessionPath, SessionRecord};

#[derive(Clone)]
struct SessionRow {
    path: SessionPath,
    label: SharedString,
    depth: usize,
    is_folder: bool,
    has_children: bool,
    expanded: bool,
}

/// 「会话」视图的状态。
pub(crate) struct SessionsState {
    tree: SessionTree,
    expanded: Vec<u64>,
    selected: Option<SessionPath>,
    rows: RowCache<SessionRow>,
}

impl SessionsState {
    pub(crate) fn new() -> Self {
        let (entries, expanded) = SessionTree::load();
        Self {
            tree: SessionTree::new(entries),
            expanded,
            selected: None,
            rows: RowCache::default(),
        }
    }

    fn persist(&self) {
        if let Err(error) = SessionTree::persist(self.tree.entries(), &self.expanded) {
            eprintln!("[sessions] 保存会话记录失败：{error}");
        }
    }

    fn rows(&mut self) -> Rc<Vec<SessionRow>> {
        if let Some(rows) = self.rows.cached() {
            return rows;
        }
        let mut out = Vec::new();
        let mut path = SessionPath::new();
        Self::flatten(self.tree.entries(), &self.expanded, &mut path, &mut out);
        self.rows.store(out)
    }

    fn flatten(
        entries: &[SessionEntry],
        expanded: &[u64],
        path: &mut SessionPath,
        out: &mut Vec<SessionRow>,
    ) {
        for (ix, entry) in entries.iter().enumerate() {
            path.push(ix);
            let is_folder = entry.is_folder();
            let is_open = is_folder && expanded.contains(&entry.id());
            out.push(SessionRow {
                path: path.clone(),
                label: entry.label().to_owned().into(),
                depth: path.len() - 1,
                is_folder,
                has_children: matches!(entry, SessionEntry::Folder(folder) if !folder.children.is_empty()),
                expanded: is_open,
            });
            if is_open && let SessionEntry::Folder(folder) = entry {
                Self::flatten(&folder.children, expanded, path, out);
            }
            path.pop();
        }
    }

    fn row(&self, ix: usize) -> Option<SessionRow> {
        self.rows.row(ix)
    }

    /// 侧边栏虚拟列表的全部内容项。
    pub(super) fn sidebar_items(&mut self, cx: &mut Context<Self>) -> Vec<SessionsItem> {
        let rows = self.rows();
        if rows.is_empty() {
            return vec![SessionsItem::Empty];
        }
        let state = cx.entity().downgrade();
        let mut items: Vec<SessionsItem> = (0..rows.len())
            .map(|ix| SessionsItem::Row {
                state: state.clone(),
                ix,
            })
            .collect();
        items.push(SessionsItem::TopLevelDrop);
        items
    }

    /// 点一行:文件夹切换展开,会话行记选中。
    fn activate_row(&mut self, path: SessionPath, is_folder: bool, cx: &mut Context<Self>) {
        self.selected = Some(path.clone());
        if is_folder && let Some(id) = self.entry(&path).map(SessionEntry::id) {
            let was_open = self.expanded.contains(&id);
            self.expanded.retain(|open| *open != id);
            if !was_open {
                self.expanded.push(id);
            }
            self.rows.bump();
            self.persist();
        }
        cx.notify();
    }

    fn entry(&self, path: &[usize]) -> Option<&SessionEntry> {
        self.tree.entry(path)
    }

    pub(crate) fn record(&self, path: &[usize]) -> Option<SessionRecord> {
        match self.entry(path)? {
            SessionEntry::Session(record) => Some(record.clone()),
            SessionEntry::Folder(_) => None,
        }
    }

    pub(crate) fn update_record(
        &mut self,
        id: u64,
        record: SessionRecord,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.tree.update_record(id, &record) {
            return false;
        }
        self.persist();
        self.rows.bump();
        cx.notify();
        true
    }

    /// 往列表里加一条记录。
    pub(crate) fn add_record(
        &mut self,
        record: SessionRecord,
        folder: Option<SessionPath>,
        cx: &mut Context<Self>,
    ) {
        let target = folder.unwrap_or_default();
        if self.tree.add_record(record, &target) {
            self.expand_folder(&target);
            self.persist();
        }
        self.rows.bump();
        cx.notify();
    }

    pub(crate) fn add_folder(
        &mut self,
        name: SharedString,
        parent: Option<SessionPath>,
        cx: &mut Context<Self>,
    ) {
        let target = parent.unwrap_or_default();
        if self.tree.add_folder(name.to_string(), &target) {
            self.expand_folder(&target);
            self.persist();
        }
        self.rows.bump();
        cx.notify();
    }

    /// 删除一个条目(文件夹连带内容)。
    pub(crate) fn remove_entry(&mut self, path: &[usize], cx: &mut Context<Self>) {
        if self.tree.remove(path) {
            self.persist();
        }
        self.rows.bump();
        cx.notify();
    }

    /// 把条目挪到 `into` 文件夹下。
    pub(crate) fn move_entry(&mut self, from: &[usize], into: &[usize], cx: &mut Context<Self>) {
        let Some(removed) = self.tree.move_entry(from, into) else {
            return;
        };
        self.expand_folder(&removed);
        self.rows.bump();
        self.persist();
        cx.notify();
    }

    fn expand_folder(&mut self, path: &[usize]) {
        if let Some(id) = self.entry(path).map(SessionEntry::id)
            && !self.expanded.contains(&id)
        {
            self.expanded.push(id);
        }
    }

    /// 按记录生成开终端参数;非会话返回 `None`。
    pub(crate) fn record_request(&self, path: &[usize]) -> Option<SessionRequest> {
        match self.entry(path) {
            Some(SessionEntry::Session(record)) => Some(record.request()),
            _ => None,
        }
    }
}

impl crate::AppRoot {
    /// 按记录开一个终端。
    pub(crate) fn open_session_record(
        &mut self,
        path: &[usize],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = self.sessions.read(cx).record_request(path) else {
            return;
        };
        self.spawn_session(request, window, cx);
    }
}

#[derive(Clone)]
pub(super) enum SessionsItem {
    Row {
        state: WeakEntity<SessionsState>,
        ix: usize,
    },
    TopLevelDrop,
    Empty,
}

impl SessionsItem {
    /// 画这一项。
    pub(super) fn render(self, cx: &mut App) -> AnyElement {
        match self {
            Self::Row { state, ix } => {
                let Ok(Some(row)) = state.read_with(&*cx, |sessions, _| sessions.row(ix)) else {
                    return div().w_full().into_any_element();
                };
                let selected = state
                    .read_with(&*cx, |sessions, _| {
                        sessions.selected.as_deref() == Some(row.path.as_slice())
                    })
                    .unwrap_or(false);
                div()
                    .w_full()
                    .child(session_row_element(ix, &row, selected, &state, cx))
                    .into_any_element()
            }
            Self::TopLevelDrop => div()
                .id("session-tree-top-level-drop")
                .w_full()
                .h(px(16.))
                .drag_over::<DragSessionEntry>(|style, _, _, cx| style.bg(cx.theme().tokens.accent))
                .on_drop(move |drag: &DragSessionEntry, window, cx| {
                    drop_into(drag.path.clone(), None, window, cx)
                })
                .into_any_element(),
            Self::Empty => empty_state(
                IconName::Inbox,
                "还没有会话",
                Some("在空白处右键可新建会话 / 新建文件夹；双击会话条目打开终端"),
            )
            .into_any_element(),
        }
    }
}

/// 拖动中的条目(拖放载荷)。
#[derive(Clone)]
struct DragSessionEntry {
    path: SessionPath,
    label: SharedString,
}

/// 把条目挪进 `into` 文件夹。
fn drop_into(from: SessionPath, into: Option<SessionPath>, window: &mut Window, cx: &mut App) {
    window.dispatch_action(Box::new(MoveEntry { from, into }), cx);
}

/// 会话树的一行(文件夹行 / 会话行)。
fn session_row_element(
    ix: usize,
    row: &SessionRow,
    selected: bool,
    state: &WeakEntity<SessionsState>,
    cx: &mut App,
) -> AnyElement {
    let label = row.label.clone();
    let payload = DragSessionEntry {
        path: row.path.clone(),
        label: label.clone(),
    };
    let drop_path = if row.is_folder {
        row.path.clone()
    } else {
        row.path[..row.path.len() - 1].to_vec()
    };
    let click_path = row.path.clone();
    let menu_path = row.path.clone();
    let is_folder = row.is_folder;
    let icon = match (row.is_folder, row.expanded) {
        (true, true) => IconName::FolderOpen,
        (true, false) => IconName::Folder,
        (false, _) => IconName::Globe,
    };
    let caret = (row.is_folder && row.has_children).then_some({
        if row.expanded {
            IconName::ChevronDown
        } else {
            IconName::ChevronRight
        }
    });

    row_shell(ix, row.depth, selected, cx)
        .cursor(CursorStyle::PointingHand)
        .on_drag(payload, |drag: &DragSessionEntry, _, _, cx| {
            cx.new(|_| DragPreview {
                label: drag.label.clone(),
            })
        })
        .drag_over::<DragSessionEntry>(|style, _, _, cx| style.bg(cx.theme().tokens.accent))
        .on_drop(move |drag: &DragSessionEntry, window, cx| {
            drop_into(drag.path.clone(), Some(drop_path.clone()), window, cx)
        })
        .child(row_content(caret, icon, label))
        .on_click({
            let state = state.clone();
            move |event, window, cx| {
                if event.click_count() >= 2 && !is_folder {
                    window.dispatch_action(
                        Box::new(OpenSession {
                            path: click_path.clone(),
                        }),
                        cx,
                    );
                    return;
                }
                let _ = state.update(cx, |sessions, cx| {
                    sessions.activate_row(click_path.clone(), is_folder, cx)
                });
            }
        })
        .context_menu(move |menu, _window, _cx| {
            if is_folder {
                menu.menu(
                    "在这里新建会话",
                    Box::new(NewSession {
                        folder: Some(menu_path.clone()),
                    }),
                )
                .menu(
                    "新建子文件夹",
                    Box::new(NewFolder {
                        parent: Some(menu_path.clone()),
                    }),
                )
                .separator()
                .menu(
                    "删除文件夹",
                    Box::new(RemoveEntry {
                        path: menu_path.clone(),
                    }),
                )
            } else {
                menu.menu(
                    "打开会话",
                    Box::new(OpenSession {
                        path: menu_path.clone(),
                    }),
                )
                .menu(
                    "编辑会话",
                    Box::new(EditSession {
                        path: menu_path.clone(),
                    }),
                )
                .separator()
                .menu(
                    "删除会话",
                    Box::new(RemoveEntry {
                        path: menu_path.clone(),
                    }),
                )
            }
        })
        .into_any_element()
}
