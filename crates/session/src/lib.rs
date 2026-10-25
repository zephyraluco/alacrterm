//! Saved session models, persistence, connection requests and tree operations.
use serde::{Deserialize, Serialize};
use terminal::{SshParams, TerminalTarget};
#[derive(Clone)]
pub struct SessionRequest {
    pub name: Option<String>,
    pub target: TerminalTarget,
}
/// 条目路径:顶层开始的下标链(`[]` = 顶层)。
pub type SessionPath = Vec<usize>;

/// 会话列表里的一个条目。
pub enum SessionEntry {
    Folder(SessionFolder),
    Session(SessionRecord),
}

impl SessionEntry {
    pub fn label(&self) -> &str {
        match self {
            Self::Folder(folder) => &folder.name,
            Self::Session(record) => &record.name,
        }
    }

    pub fn id(&self) -> u64 {
        match self {
            Self::Folder(folder) => folder.id,
            Self::Session(record) => record.id,
        }
    }

    pub fn is_folder(&self) -> bool {
        matches!(self, Self::Folder(_))
    }
}

pub struct SessionFolder {
    pub id: u64,
    pub name: String,
    pub children: Vec<SessionEntry>,
}

impl SessionFolder {
    pub fn new(id: u64, name: String) -> Self {
        Self {
            id,
            name,
            children: Vec::new(),
        }
    }
}

/// 一条会话记录(连接参数)。
#[derive(Clone)]
pub struct SessionRecord {
    pub id: u64,
    pub name: String,
    pub user: String,
    pub host: String,
    pub port: u16,
    pub password: Option<String>,
}

impl SessionRecord {
    /// 这条记录的会话参数。
    pub fn request(&self) -> SessionRequest {
        let params = SshParams::new(self.host.clone(), self.port, self.user.clone());
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
    },
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct PersistedSessions {
    entries: Vec<PersistedEntry>,
    expanded: Vec<u64>,
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
            },
        }
    }
}

impl From<PersistedEntry> for SessionEntry {
    fn from(entry: PersistedEntry) -> Self {
        match entry {
            PersistedEntry::Folder { id, name, children } => Self::Folder(SessionFolder {
                id,
                name,
                children: children.into_iter().map(Self::from).collect(),
            }),
            PersistedEntry::Session {
                id,
                name,
                user,
                host,
                port,
                password,
            } => Self::Session(SessionRecord {
                id,
                name,
                user,
                host,
                port,
                password,
            }),
        }
    }
}

pub struct SessionTree {
    entries: Vec<SessionEntry>,
    next_id: u64,
}
impl SessionTree {
    pub fn entries(&self) -> &[SessionEntry] {
        &self.entries
    }
    pub fn update_record(&mut self, id: u64, record: &SessionRecord) -> bool {
        Self::replace_record(&mut self.entries, id, record)
    }

    pub fn new(entries: Vec<SessionEntry>) -> Self {
        let next_id = Self::max_id(&entries);
        Self { entries, next_id }
    }
    pub fn entry(&self, path: &[usize]) -> Option<&SessionEntry> {
        let (index, parent) = path.split_last()?;
        let mut entries = &self.entries;
        for step in parent {
            entries = match entries.get(*step)? {
                SessionEntry::Folder(folder) => &folder.children,
                SessionEntry::Session(_) => return None,
            };
        }
        entries.get(*index)
    }
    pub fn add_record(&mut self, mut record: SessionRecord, parent: &[usize]) -> bool {
        self.next_id += 1;
        record.id = self.next_id;
        let Some(children) = Self::children_mut(&mut self.entries, parent) else {
            return false;
        };
        children.push(SessionEntry::Session(record));
        true
    }
    pub fn add_folder(&mut self, name: String, parent: &[usize]) -> bool {
        self.next_id += 1;
        let Some(children) = Self::children_mut(&mut self.entries, parent) else {
            return false;
        };
        children.push(SessionEntry::Folder(SessionFolder::new(self.next_id, name)));
        true
    }
    pub fn remove(&mut self, path: &[usize]) -> bool {
        Self::take_entry(&mut self.entries, path).is_some()
    }
    /// Return the destination's adjusted path after removing the source.
    pub fn move_entry(&mut self, from: &[usize], into: &[usize]) -> Option<SessionPath> {
        if from.is_empty()
            || into.starts_with(from)
            || self.entry(from).is_none()
            || !(into.is_empty() || matches!(self.entry(into), Some(SessionEntry::Folder(_))))
        {
            return None;
        }
        let destination = Self::shift_path_after_removal(into.to_vec(), from);
        let entry = Self::take_entry(&mut self.entries, from)?;
        Self::children_mut(&mut self.entries, &destination)?.push(entry);
        Some(destination)
    }

    pub fn load() -> (Vec<SessionEntry>, Vec<u64>) {
        let path = settings::sessions_config_path();
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

    pub fn persist(entries: &[SessionEntry], expanded: &[u64]) {
        let path = settings::sessions_config_path();
        let stored = PersistedSessions {
            entries: entries.iter().map(PersistedEntry::from).collect(),
            expanded: expanded.to_vec(),
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

    /// Locate by stable ID, since a tree path may change while the dialog is open.
    fn replace_record(entries: &mut [SessionEntry], id: u64, record: &SessionRecord) -> bool {
        for entry in entries {
            match entry {
                SessionEntry::Session(existing) if existing.id == id => {
                    *existing = record.clone();
                    existing.id = id;
                    return true;
                }
                SessionEntry::Folder(folder) => {
                    if Self::replace_record(&mut folder.children, id, record) {
                        return true;
                    }
                }
                _ => {}
            }
        }
        false
    }

    fn take_entry(entries: &mut Vec<SessionEntry>, path: &[usize]) -> Option<SessionEntry> {
        let (ix, parent) = path.split_last()?;
        let children = Self::children_mut(entries, parent)?;
        (*ix < children.len()).then(|| children.remove(*ix))
    }

    /// 取出 `removed` 后修正路径下标。
    fn shift_path_after_removal(mut path: SessionPath, removed: &[usize]) -> SessionPath {
        let Some((removed_ix, parent)) = removed.split_last() else {
            return path;
        };
        if path.len() > parent.len() && path.starts_with(parent) && path[parent.len()] > *removed_ix
        {
            path[parent.len()] -= 1;
        }
        path
    }
}
#[cfg(test)]
mod tests {
    use super::{PersistedEntry, SessionEntry, SessionFolder, SessionRecord};

    #[test]
    fn moving_into_later_sibling_adjusts_path_and_rejects_cycles() {
        let mut tree = super::SessionTree::new(vec![
            SessionEntry::Folder(SessionFolder::new(1, "first".into())),
            SessionEntry::Folder(SessionFolder::new(2, "second".into())),
        ]);
        assert_eq!(tree.move_entry(&[0], &[1]), Some(vec![0]));
        assert!(tree.entry(&[0, 0]).is_some());
        assert!(tree.move_entry(&[0], &[0, 0]).is_none());
        tree.add_folder("third".into(), &[]);
        assert_eq!(tree.entry(&[1]).unwrap().id(), 3);
        assert!(!tree.remove(&[99]));
    }
    #[test]
    fn editing_finds_moved_record_by_id_and_preserves_neighbors() {
        let make = |id, host: &str| SessionRecord {
            id,
            name: "server".into(),
            user: "user".into(),
            host: host.into(),
            port: 22,
            password: Some(" old password ".into()),
        };
        let mut entries = vec![SessionEntry::Folder(SessionFolder {
            id: 1,
            name: "group".into(),
            children: vec![
                SessionEntry::Session(make(2, "neighbor")),
                SessionEntry::Session(make(3, "before")),
            ],
        })];
        let mut replacement = make(0, "after");
        replacement.port = 2222;
        replacement.password = None;
        assert!(super::SessionTree::replace_record(
            &mut entries,
            3,
            &replacement
        ));
        let SessionEntry::Folder(folder) = &entries[0] else {
            panic!("expected folder");
        };
        let SessionEntry::Session(neighbor) = &folder.children[0] else {
            panic!("expected session");
        };
        let SessionEntry::Session(edited) = &folder.children[1] else {
            panic!("expected session");
        };
        assert_eq!(neighbor.host, "neighbor");
        assert_eq!(edited.id, 3);
        assert_eq!(edited.host, "after");
        assert_eq!(edited.port, 2222);
        assert!(edited.password.is_none());
        assert!(!super::SessionTree::replace_record(
            &mut entries,
            99,
            &replacement
        ));
        let stored = serde_json::to_string(&PersistedEntry::from(&entries[0])).unwrap();
        assert!(stored.contains("after"));
        assert!(!stored.contains("before"));
    }

    #[test]
    fn old_x11_setting_is_ignored_and_no_longer_persisted() {
        for enabled in [false, true] {
            let json = format!(
                r#"{{"type":"session","id":1,"name":"old","user":"me","host":"localhost","port":22,"x11_forwarding":{enabled}}}"#
            );
            let stored: PersistedEntry = serde_json::from_str(&json).unwrap();
            let entry = SessionEntry::from(stored);
            let json = serde_json::to_string(&PersistedEntry::from(&entry)).unwrap();
            assert!(!json.contains("x11_forwarding"));
        }
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

        assert_eq!(record.password.as_deref(), Some("saved password"));
    }
}
