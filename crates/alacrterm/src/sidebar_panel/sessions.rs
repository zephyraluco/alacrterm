//! 「会话」视图:文件夹形式的会话记录树,入口 [`SessionsState`]。
//! 行骨架与摊平走 [`super::shared`],记录持久化到 `config/sessions.json`。

use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, Context, CursorStyle, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity,
    Window, div, px,
};
use gpui_kit::component::{ActiveTheme as _, menu::ContextMenuExt as _};
use serde::{Deserialize, Serialize};

use super::empty_state;
use super::shared::{DragPreview, RowCache, row_content, row_shell};
use crate::actions::{MoveEntry, NewFolder, NewSession, OpenSession, RemoveEntry};
use crate::assets::IconName;
use crate::terminal_panel::SessionRequest;
use terminal::{SshParams, TerminalTarget};


/// 条目路径:顶层开始的下标链(`[]` = 顶层)。
pub(crate) type SessionPath = Vec<usize>;

/// 会话列表里的一个条目。
pub(crate) enum SessionEntry {
    Folder(SessionFolder),
    Session(SessionRecord),
}

impl SessionEntry {
    pub(crate) fn label(&self) -> SharedString {
        match self {
            Self::Folder(folder) => folder.name.clone(),
            Self::Session(record) => record.name.clone(),
        }
    }

    pub(crate) fn id(&self) -> u64 {
        match self {
            Self::Folder(folder) => folder.id,
            Self::Session(record) => record.id,
        }
    }

    pub(crate) fn is_folder(&self) -> bool {
        matches!(self, Self::Folder(_))
    }
}

pub(crate) struct SessionFolder {
    pub(crate) id: u64,
    pub(crate) name: SharedString,
    pub(crate) children: Vec<SessionEntry>,
}

impl SessionFolder {
    pub(crate) fn new(id: u64, name: SharedString) -> Self {
        Self {
            id,
            name,
            children: Vec::new(),
        }
    }
}

/// 一条会话记录(连接参数)。
pub(crate) struct SessionRecord {
    pub(crate) id: u64,
    pub(crate) name: SharedString,
    pub(crate) user: String,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) password: Option<String>,
    pub(crate) x11_forwarding: bool,
}

impl SessionRecord {
    /// 这条记录的会话参数。
    pub(crate) fn request(&self) -> SessionRequest {
        let mut params = SshParams::new(self.host.clone(), self.port, self.user.clone());
        params.x11_forwarding = self.x11_forwarding;
        let params = match &self.password {
            Some(password) => params.with_password(password.clone()),
            None => params,
        };
        SessionRequest {
            name: Some(self.name.clone()),
            target: TerminalTarget::Ssh(params),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PersistedEntry {
    Folder {
        id: u64,
        name: String,
        children: Vec<PersistedEntry>,
    },
    Session {
        id: u64,
        name: String,
        user: String,
        host: String,
        port: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        password: Option<String>,
        #[serde(default)]
        x11_forwarding: bool,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct PersistedSessions {
    entries: Vec<PersistedEntry>,
    expanded: Vec<u64>,
}

impl Default for PersistedSessions {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            expanded: Vec::new(),
        }
    }
}

impl From<&SessionEntry> for PersistedEntry {
    fn from(entry: &SessionEntry) -> Self {
        match entry {
            SessionEntry::Folder(folder) => Self::Folder {
                id: folder.id,
                name: folder.name.to_string(),
                children: folder.children.iter().map(Self::from).collect(),
            },
            SessionEntry::Session(record) => Self::Session {
                id: record.id,
                name: record.name.to_string(),
                user: record.user.clone(),
                host: record.host.clone(),
                port: record.port,
                password: record.password.clone(),
                x11_forwarding: record.x11_forwarding,
            },
        }
    }
}

impl From<PersistedEntry> for SessionEntry {
    fn from(entry: PersistedEntry) -> Self {
        match entry {
            PersistedEntry::Folder { id, name, children } => Self::Folder(SessionFolder {
                id,
                name: name.into(),
                children: children.into_iter().map(Self::from).collect(),
            }),
            PersistedEntry::Session {
                id,
                name,
                user,
                host,
                port,
                password,
                x11_forwarding,
            } => Self::Session(SessionRecord {
                id,
                name: name.into(),
                user,
                host,
                port,
                password,
                x11_forwarding,
            }),
        }
    }
}


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
    entries: Vec<SessionEntry>,
    expanded: Vec<u64>,
    selected: Option<SessionPath>,
    rows: RowCache<SessionRow>,
    next_id: u64,
}

impl SessionsState {
    pub(crate) fn new() -> Self {
        let (entries, expanded) = Self::load();
        let next_id = Self::max_id(&entries);
        Self {
            entries,
            expanded,
            selected: None,
            rows: RowCache::default(),
            next_id,
        }
    }

    fn load() -> (Vec<SessionEntry>, Vec<u64>) {
        let path = crate::config::sessions_config_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return (Vec::new(), Vec::new());
            }
            Err(error) => {
                eprintln!("[sessions] 读取会话记录 {} 失败：{error}", path.display());
                return (Vec::new(), Vec::new());
            }
        };

        match serde_json::from_str::<PersistedSessions>(&text) {
            Ok(stored) => (
                stored.entries.into_iter().map(Into::into).collect(),
                stored.expanded,
            ),
            Err(error) => {
                eprintln!("[sessions] 解析会话记录 {} 失败：{error}", path.display());
                (Vec::new(), Vec::new())
            }
        }
    }

    fn max_id(entries: &[SessionEntry]) -> u64 {
        entries
            .iter()
            .map(|entry| match entry {
                SessionEntry::Folder(folder) => folder.id.max(Self::max_id(&folder.children)),
                SessionEntry::Session(record) => record.id,
            })
            .max()
            .unwrap_or_default()
    }

    fn persist(&self) {
        let path = crate::config::sessions_config_path();
        let stored = PersistedSessions {
            entries: self.entries.iter().map(PersistedEntry::from).collect(),
            expanded: self.expanded.clone(),
        };

        let result = serde_json::to_string_pretty(&stored)
            .map_err(std::io::Error::other)
            .and_then(|mut text| {
                text.push('\n');
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, text)
            });
        if let Err(error) = result {
            eprintln!("[sessions] 保存会话记录 {} 失败：{error}", path.display());
        }
    }

    fn alloc_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn rows(&mut self) -> Rc<Vec<SessionRow>> {
        if let Some(rows) = self.rows.cached() {
            return rows;
        }
        let mut out = Vec::new();
        let mut path = SessionPath::new();
        Self::flatten(&self.entries, &self.expanded, &mut path, &mut out);
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
                label: entry.label(),
                depth: path.len() - 1,
                is_folder,
                has_children: matches!(entry, SessionEntry::Folder(folder) if !folder.children.is_empty()),
                expanded: is_open,
            });
            if is_open
                && let SessionEntry::Folder(folder) = entry
            {
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
        if is_folder
            && let Some(id) = self.entry(&path).map(SessionEntry::id)
        {
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
        let (ix, parent) = path.split_last()?;
        let mut entries: &[SessionEntry] = &self.entries;
        for step in parent {
            entries = match entries.get(*step)? {
                SessionEntry::Folder(folder) => &folder.children,
                SessionEntry::Session(_) => return None,
            };
        }
        entries.get(*ix)
    }

    fn children_mut<'a>(
        entries: &'a mut Vec<SessionEntry>,
        path: &[usize],
    ) -> Option<&'a mut Vec<SessionEntry>> {
        match path.split_first() {
            None => Some(entries),
            Some((ix, rest)) => match entries.get_mut(*ix)? {
                SessionEntry::Folder(folder) => Self::children_mut(&mut folder.children, rest),
                SessionEntry::Session(_) => None,
            },
        }
    }

    /// 往列表里加一条记录。
    pub(crate) fn add_record(
        &mut self,
        mut record: SessionRecord,
        folder: Option<SessionPath>,
        cx: &mut Context<Self>,
    ) {
        record.id = self.alloc_id();
        let target = folder.unwrap_or_default();
        if let Some(children) = Self::children_mut(&mut self.entries, &target) {
            children.push(SessionEntry::Session(record));
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
        let id = self.alloc_id();
        let target = parent.unwrap_or_default();
        if let Some(children) = Self::children_mut(&mut self.entries, &target) {
            children.push(SessionEntry::Folder(SessionFolder::new(id, name)));
            self.expand_folder(&target);
            self.persist();
        }
        self.rows.bump();
        cx.notify();
    }

    /// 删除一个条目(文件夹连带内容)。
    pub(crate) fn remove_entry(&mut self, path: &[usize], cx: &mut Context<Self>) {
        let Some((ix, parent)) = path.split_last() else {
            return;
        };
        let mut removed = false;
        if let Some(children) = Self::children_mut(&mut self.entries, parent)
            && *ix < children.len()
        {
            children.remove(*ix);
            removed = true;
        }
        if removed {
            self.persist();
        }
        self.rows.bump();
        cx.notify();
    }

    /// 把条目挪到 `into` 文件夹下。
    pub(crate) fn move_entry(&mut self, from: &[usize], into: &[usize], cx: &mut Context<Self>) {
        if from.is_empty()
            || into.starts_with(from)
            || !self.folder_exists(into)
            || self.entry(from).is_none()
        {
            return;
        }

        let removed = Self::shift_path_after_removal(into.to_vec(), from);
        let entry = Self::take_entry(&mut self.entries, from).expect("刚刚确认过这条路径有条目");
        let destination =
            Self::children_mut(&mut self.entries, &removed).expect("目标文件夹刚刚校验过");
        destination.push(entry);
        self.expand_folder(&removed);
        self.rows.bump();
        self.persist();
        cx.notify();
    }

    fn take_entry(entries: &mut Vec<SessionEntry>, path: &[usize]) -> Option<SessionEntry> {
        let (ix, parent) = path.split_last()?;
        let children = Self::children_mut(entries, parent)?;
        (*ix < children.len()).then(|| children.remove(*ix))
    }

    fn folder_exists(&self, path: &[usize]) -> bool {
        path.is_empty() || matches!(self.entry(path), Some(SessionEntry::Folder(_)))
    }

    fn expand_folder(&mut self, path: &[usize]) {
        if let Some(id) = self.entry(path).map(SessionEntry::id)
            && !self.expanded.contains(&id)
        {
            self.expanded.push(id);
        }
    }

    /// 取出 `removed` 后修正路径下标。
    fn shift_path_after_removal(mut path: SessionPath, removed: &[usize]) -> SessionPath {
        let (removed_ix, parent) = removed.split_last().expect("removed 不会是空路径");
        if path.len() > parent.len() && path.starts_with(parent) && path[parent.len()] > *removed_ix {
            path[parent.len()] -= 1;
        }
        path
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
    let caret = (row.is_folder && row.has_children).then(|| {
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

#[cfg(test)]
mod tests {
    use super::{PersistedEntry, SessionEntry, SessionFolder, SessionRecord};

    #[test]
    fn legacy_sessions_default_to_no_x11_forwarding() {
        let record: PersistedEntry = serde_json::from_str(
            r#"{"type":"session","id":1,"name":"old","user":"me","host":"localhost","port":22}"#,
        )
        .unwrap();
        let SessionEntry::Session(record) = SessionEntry::from(record) else {
            panic!("expected session")
        };
        assert!(!record.x11_forwarding);
        let terminal::TerminalTarget::Ssh(params) = record.request().target else {
            panic!("expected SSH")
        };
        assert!(!params.x11_forwarding);
    }

    #[test]
    fn persisted_ssh_session_round_trips_with_password() {
        let entry = SessionEntry::Folder(SessionFolder {
            id: 4,
            name: "Production".into(),
            children: vec![SessionEntry::Session(SessionRecord {
                id: 5,
                name: "Database".into(),
                user: "admin".into(),
                host: "db.example.com".into(),
                port: 2222,
                password: Some("saved password".into()),
                x11_forwarding: true,
            })],
        });

        let json = serde_json::to_string(&PersistedEntry::from(&entry))
            .expect("session record should serialize");
        assert!(json.contains("saved password"));

        let restored: PersistedEntry =
            serde_json::from_str(&json).expect("session record should deserialize");
        let SessionEntry::Folder(folder) = SessionEntry::from(restored) else {
            panic!("folder should be restored");
        };
        let SessionEntry::Session(record) = &folder.children[0] else {
            panic!("SSH session should be restored");
        };

        assert_eq!(folder.name, "Production");
        assert_eq!(record.host, "db.example.com");
        assert_eq!(record.port, 2222);
        assert!(record.x11_forwarding);
        let terminal::TerminalTarget::Ssh(params) = record.request().target else {
            panic!("expected SSH")
        };
        assert!(params.x11_forwarding);
        assert_eq!(record.password.as_deref(), Some("saved password"));
    }
}
