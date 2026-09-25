//! 「会话」视图:文件夹形式的**记录树**(MobaXterm 那种形态),状态收在 [`SessionsState`]。
//!
//! 列表内容是**会话记录**(连接配置),与终端实例无关:关掉终端不影响列表,也没有自动生成的
//! 根文件夹(顶层就是用户建的文件夹与记录)。入口:状态栏左下角建文件夹、右下角新建会话,
//! 文件夹行右键可再建会话 / 子文件夹,右键删除(文件夹连带内容),**双击记录行开终端**
//! ([`OpenSession`]);记录与文件夹都能**拖动**,落点合法性由 [`SessionsState::move_entry`]
//! 把关。
//!
//! ⚠️ 虚拟化、摊平缓存与行骨架都走 [`super::shared`](与「文件管理器」共用);本模块只管
//! 记录模型 + 展开状态 + 摊平([`SessionsItem`] 一行一项,动机见 `files` 模块文档)。
//!
//! 行交互一律派发 action([`OpenSession`] / [`RemoveEntry`] / …):行元素只拿得到 `&mut App`。

use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, Context, CursorStyle, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity,
    Window, div, px,
};
use gpui_kit::component::{ActiveTheme as _, menu::ContextMenuExt as _};

use super::empty_state;
use super::shared::{DragPreview, RowCache, row_content, row_shell};
use crate::actions::{MoveEntry, NewFolder, NewSession, OpenSession, RemoveEntry};
use crate::assets::IconName;
use crate::terminal_panel::SessionRequest;
use terminal::{SshParams, TerminalTarget};

// ---------------------------------------------------------------- 记录模型

/// 会话列表里一个条目的路径：从顶层开始的下标链（`[]` = 顶层本身）。
pub(crate) type SessionPath = Vec<usize>;

/// 侧边栏「会话」列表里的一个条目（顺序 = 列表里的显示顺序，可以嵌套）。
///
/// 列表内容是**记录**（配置），与终端实例完全无关：一条记录有没有在跑终端，取决于
/// 用户是否双击过它；文件夹只用来分组，由用户自己创建（**不会自动生成**，行上也不显示
/// 「里面有几条」）。整体形态参考 MobaXterm 的会话树。
pub(crate) enum SessionEntry {
    /// 用户建的文件夹（可以再套文件夹）。
    Folder(SessionFolder),
    /// 一条 SSH 会话记录。
    Session(SessionRecord),
}

impl SessionEntry {
    /// 行标题（文件夹名 / 会话名）。
    pub(crate) fn label(&self) -> SharedString {
        match self {
            Self::Folder(folder) => folder.name.clone(),
            Self::Session(record) => record.name.clone(),
        }
    }

    /// 这一项的稳定 id：展开状态按它记，所以增删 / 移动都不用修正任何下标。
    pub(crate) fn id(&self) -> u64 {
        match self {
            Self::Folder(folder) => folder.id,
            Self::Session(record) => record.id,
        }
    }

    /// 这一项是文件夹吗（⚠️ 与「有没有子项」是两回事：空文件夹也是文件夹，
    /// 画不画 caret 由摊平时的 `SessionRow::has_children` 决定）。
    pub(crate) fn is_folder(&self) -> bool {
        matches!(self, Self::Folder(_))
    }
}

/// 会话列表里的一个文件夹。
pub(crate) struct SessionFolder {
    /// 稳定 id（由 [`SessionsState::add_folder`] 分配）。
    pub(crate) id: u64,
    /// 文件夹名（行标题）。
    pub(crate) name: SharedString,
    /// 子条目（文件夹 / 记录混排，顺序即显示顺序）。
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

/// 会话列表里的一条**会话记录**：只保存连接参数（只在内存里，重启不保留）。
pub(crate) struct SessionRecord {
    /// 稳定 id（由 [`SessionsState::add_record`] 分配；构造时先填 0）。
    pub(crate) id: u64,
    /// 显示名（列表里的行标题，也是打开后标签页的名字）。
    pub(crate) name: SharedString,
    /// 登录用户名。
    pub(crate) user: String,
    /// 主机地址（IP / 域名）。
    pub(crate) host: String,
    /// SSH 端口。
    pub(crate) port: u16,
    /// 登录密码（**只在内存里**，不落盘）；`None` = 只用 ssh-agent 与 `~/.ssh` 里的私钥。
    pub(crate) password: Option<String>,
}

impl SessionRecord {
    /// 打开这条记录要用的会话参数（见 [`crate::terminal_panel::SessionRequest`]）。
    ///
    /// 连接由**内建 SSH 客户端**完成（不再拉起外部 `ssh` 命令），主机密钥默认按
    /// `ask` 处理：首次连接会弹窗让用户核对指纹。
    pub(crate) fn request(&self) -> SessionRequest {
        let params = SshParams::new(self.host.clone(), self.port, self.user.clone());
        // 公钥(agent / 私钥文件)永远先试,密码只在它失败后兜底 —— 与 OpenSSH 一致。
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

// ---------------------------------------------------------------- 视图状态

/// 摊平后的一行(交给侧边栏的虚拟列表)。
#[derive(Clone)]
struct SessionRow {
    /// 从顶层开始的下标链:既定位条目,也是展开状态的键。
    path: SessionPath,
    label: SharedString,
    /// 缩进层级(顶层是 0)。
    depth: usize,
    is_folder: bool,
    /// 文件夹有子项时才画 caret(与「点了会不会展开」无关:空文件夹点了也没反应)。
    has_children: bool,
    /// 文件夹当前是否展开(会话行恒 `false`)。
    expanded: bool,
}

/// 「会话」视图(记录树)的状态:记录模型 + 展开状态 + 摊平后的行清单。
///
/// **数据与交互状态都在这里**;渲染不在这里 —— [`SessionsState::sidebar_items`] 把
/// 「要摆哪些项」交给侧边栏(它负责虚拟化)。
pub(crate) struct SessionsState {
    /// 「会话」列表的条目树(顺序 = 显示顺序)。
    entries: Vec<SessionEntry>,
    /// 展开着的文件夹(按 [`SessionEntry::id`] 记 ⇒ 增删 / 移动都不用修正下标)。
    expanded: Vec<u64>,
    /// 最近点中的那一行(只做高亮)。
    selected: Option<SessionPath>,
    /// 摊平后的行清单缓存(见 [`RowCache`]:侧边栏每帧都会来要,不缓存就是每帧 `O(条目数)`)。
    rows: RowCache<SessionRow>,
    /// 下一个可用的条目 id(自增;文件夹与记录共用一个序列)。
    next_id: u64,
}

impl SessionsState {
    pub(crate) fn new() -> Self {
        Self {
            entries: Vec::new(),
            expanded: Vec::new(),
            selected: None,
            rows: RowCache::default(),
            next_id: 0,
        }
    }

    /// 分配一个新的条目 id。
    fn alloc_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// 摊平后的行清单(见 [`RowCache`];侧边栏每帧都会来要一次)。
    fn rows(&mut self) -> Rc<Vec<SessionRow>> {
        if let Some(rows) = self.rows.cached() {
            return rows;
        }
        let mut out = Vec::new();
        let mut path = SessionPath::new();
        Self::flatten(&self.entries, &self.expanded, &mut path, &mut out);
        self.rows.store(out)
    }

    /// [`SessionsState::rows`] 的递归实现:按展开状态把条目树摊成一行行。
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
            // 展开的文件夹要接着摊它的子项。
            if is_open
                && let SessionEntry::Folder(folder) = entry
            {
                Self::flatten(&folder.children, expanded, path, out);
            }
            path.pop();
        }
    }

    /// 按行号取一行(行数刚变过时外层可能还在渲染旧下标 ⇒ 取不到就返 `None`)。
    fn row(&self, ix: usize) -> Option<SessionRow> {
        self.rows.row(ix)
    }

    /// 交给侧边栏虚拟列表的全部内容项:每一行 + 列表末尾那条「拖到顶层」落点。
    ///
    /// 一条条目都没有时只摆空占位(见 [`empty_state`])。
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
        // 末尾补一条**拖到顶层**的空白落点:条目拖出文件夹后要有地方可放,
        // 顺带让最后一行下面留一点呼吸空间。
        items.push(SessionsItem::TopLevelDrop);
        items
    }

    /// 点一行:文件夹展开 / 收起,会话行只记选中(开终端是**双击**,见行渲染)。
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
        }
        cx.notify();
    }

    /// 按路径取条目（`[]` 是顶层本身，取不到 ⇒ `None`）。
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

    /// 按路径取**子条目列表**（用于往里面增删）。
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

    /// 往列表里加一条记录（「新建会话」对话框的「添加」）。
    ///
    /// `folder` = 落在哪个文件夹里（`None` = 顶层）；**只加记录、不开终端**：
    /// 记录是配置，用户双击它才会按它建终端。
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
            // 落进文件夹时顺手展开它，否则新条目看不见。
            self.expand_folder(&target);
        }
        self.rows.bump();
        cx.notify();
    }

    /// 新建一个文件夹（`parent` = 建在哪个文件夹下，`None` = 顶层）。
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
        }
        self.rows.bump();
        cx.notify();
    }

    /// 删除一个条目（记录 / 文件夹）。
    ///
    /// 文件夹**连带**里面的内容一起删；已经用它开出来的终端不受影响。
    pub(crate) fn remove_entry(&mut self, path: &[usize], cx: &mut Context<Self>) {
        let Some((ix, parent)) = path.split_last() else {
            return;
        };
        if let Some(children) = Self::children_mut(&mut self.entries, parent)
            && *ix < children.len()
        {
            // 展开标记按条目 id 记 ⇒ 删掉一项后不用担心下标串位（残留的 id 只会被忽略）。
            children.remove(*ix);
        }
        self.rows.bump();
        cx.notify();
    }

    /// 把一个条目挪到 `into` 这个文件夹下（拖放放下）：`into` 为空 = 顶层。
    ///
    /// 追加到目标目录末尾（不做行间插入 —— 用户要的是「拖到别的文件夹下」）。
    /// ⚠️ 展开状态按条目 id 记，所以搬动子树不用跟着搬任何标记。
    pub(crate) fn move_entry(&mut self, from: &[usize], into: &[usize], cx: &mut Context<Self>) {
        // 四种无效情况：没路径（顶层本身）、目标就是自己、目标是自己的子孙（会拖成环）、
        // 源条目已经不存在。全部先校验，之后再动模型（保证失败时什么都不改）。
        if from.is_empty()
            || into.starts_with(from)
            || !self.folder_exists(into)
            || self.entry(from).is_none()
        {
            return;
        }

        // 目标路径得按「取出源之后」的下标算（同一个目录里时会差一位）。
        let removed = Self::shift_path_after_removal(into.to_vec(), from);
        let entry = Self::take_entry(&mut self.entries, from).expect("刚刚确认过这条路径有条目");
        let destination =
            Self::children_mut(&mut self.entries, &removed).expect("目标文件夹刚刚校验过");
        destination.push(entry);
        // 目标目录本身也展开，否则刚拖进去的条目看不见。
        self.expand_folder(&removed);
        self.rows.bump();
        cx.notify();
    }

    /// 按路径取走一个条目（用于拖动搬家）。
    fn take_entry(entries: &mut Vec<SessionEntry>, path: &[usize]) -> Option<SessionEntry> {
        let (ix, parent) = path.split_last()?;
        let children = Self::children_mut(entries, parent)?;
        (*ix < children.len()).then(|| children.remove(*ix))
    }

    /// 路径 `path` 指向的目录存在吗（`[]` = 顶层，恒存在）。
    fn folder_exists(&self, path: &[usize]) -> bool {
        path.is_empty() || matches!(self.entry(path), Some(SessionEntry::Folder(_)))
    }

    /// 把一个文件夹标记为展开（新条目落进它 / 拖进去时用）。
    fn expand_folder(&mut self, path: &[usize]) {
        if let Some(id) = self.entry(path).map(SessionEntry::id)
            && !self.expanded.contains(&id)
        {
            self.expanded.push(id);
        }
    }

    /// `removed` 位置的条目被取走 / 删掉后，把 `path` 里受影响的下标前移一位。
    ///
    /// 只有「与 `removed` 同一个父目录、且排在它后面」的路径会变。
    fn shift_path_after_removal(mut path: SessionPath, removed: &[usize]) -> SessionPath {
        let (removed_ix, parent) = removed.split_last().expect("removed 不会是空路径");
        if path.len() > parent.len() && path.starts_with(parent) && path[parent.len()] > *removed_ix {
            path[parent.len()] -= 1;
        }
        path
    }

    /// 按一条记录生成「开终端」的参数（双击记录行 / 右键「打开会话」）。
    ///
    /// 文件夹行（或已被删掉的路径）⇒ `None`。**只取参数、不建终端**：
    /// 建终端是 [`crate::terminal_panel`] 的事，跨组件那一步留在根视图
    /// （[`crate::AppRoot::open_session_record`]）。
    pub(crate) fn record_request(&self, path: &[usize]) -> Option<SessionRequest> {
        match self.entry(path) {
            Some(SessionEntry::Session(record)) => Some(record.request()),
            _ => None,
        }
    }
}

impl crate::AppRoot {
    /// 按一条记录开一个终端（双击记录行 / 右键「打开会话」）。
    ///
    /// 同一条记录可以开任意多个终端；记录本身不受影响。新终端与「标签栏 `+`」开的
    /// 本地终端走同一条创建路径（[`AppRoot::spawn_session`]），所以分屏时的落点规则也一样。
    /// 这里是**跨组件**的那一步：会话记录（[`SessionsState`]）→ 终端实例（[`AppRoot`]）。
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

// ---------------------------------------------------------------- 侧边栏内容项

/// 「会话」视图交给侧边栏虚拟列表的一项。
///
/// ⚠️ **一行一项**:侧边栏内容区是它自己的虚拟列表,只渲染可见区间 + overdraw;
/// 再嵌一个内层虚拟列表的话,内层拿到的可用高度会等于整棵树 ⇒ 虚拟化失效(见模块文档)。
#[derive(Clone)]
pub(super) enum SessionsItem {
    /// 一条会话 / 文件夹(`ix` 是行清单下标,渲染时按需读)。
    Row {
        state: WeakEntity<SessionsState>,
        ix: usize,
    },
    /// 列表末尾那条「拖到顶层」的空白落点(条目拖出文件夹后要有地方可放)。
    TopLevelDrop,
    /// 一条条目都没有:空占位。
    Empty,
}

impl SessionsItem {
    /// 画这一项。`cx` 是侧边栏的 `&mut App`(读行数据 / 派发 action 都在这里)。
    pub(super) fn render(self, cx: &mut App) -> AnyElement {
        match self {
            Self::Row { state, ix } => {
                // 行数刚变过时外层可能还在渲染旧下标 ⇒ 取不到就摆个空元素。
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
            // 「拖到顶层」的落点:纯空白,只接拖放。
            Self::TopLevelDrop => div()
                .id("session-tree-top-level-drop")
                .w_full()
                .h(px(16.))
                .drag_over::<DragSessionEntry>(|style, _, _, cx| {
                    style.bg(cx.theme().tokens.accent)
                })
                .on_drop(move |drag: &DragSessionEntry, window, cx| {
                    drop_into(drag.path.clone(), None, window, cx)
                })
                .into_any_element(),
            Self::Empty => empty_state(
                IconName::Inbox,
                "还没有会话",
                Some("左下角 + 新建文件夹、右下角 + 新建会话；双击会话条目打开终端"),
            )
            .into_any_element(),
        }
    }
}

/// 拖动中的会话条目（拖放载荷：从哪儿拖的 + 显示名，显示名给拖拽预览用）。
#[derive(Clone)]
struct DragSessionEntry {
    /// 被拖动的条目路径。
    path: SessionPath,
    /// 预览卡片上的文字。
    label: SharedString,
}

/// 放下：把条目挪进 `into` 这个文件夹（`None` = 顶层），动作交给
/// [`MoveEntry`](crate::actions::MoveEntry) 统一处理（合法性校验也在那边）。
fn drop_into(from: SessionPath, into: Option<SessionPath>, window: &mut Window, cx: &mut App) {
    window.dispatch_action(Box::new(MoveEntry { from, into }), cx);
}

/// 会话树的一行（文件夹行 / 会话行）。
///
/// **同一目录下的文件夹与会话是同级**：两行都从 `pl(层级缩进)` 开始，会话行不再额外
/// 缩进一个 caret 的宽度（否则会话看上去像比同目录的文件夹低一级）。
/// - **文件夹行**：有子项时画 caret（展开 / 收起由行点击处理）；
/// - **会话行**：**单击只选中，双击才建终端**
///   （`OpenSession` → [`crate::AppRoot::open_session_record`]）
///   ——列表是记录，终端是实例，两者刻意分开。
///
/// **两行都可以拖动**（载荷 [`DragSessionEntry`]）：拖到文件夹行上 = 放进那个文件夹，
/// 拖到会话行上 = 放进它所在的那个目录，拖到列表下方的空白条 = 提到顶层；
/// 落点合法性（不能拖进自己 / 自己的子孙）由 [`SessionsState::move_entry`] 把关。
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
    // 拖到这一行上 = 放进它（文件夹）/ 放进它所在的目录（会话行：同级末尾）。
    let drop_path = if row.is_folder {
        row.path.clone()
    } else {
        row.path[..row.path.len() - 1].to_vec()
    };
    // 两个闭包各持一份路径（`move` 只能移动一次）。
    let click_path = row.path.clone();
    let menu_path = row.path.clone();
    let is_folder = row.is_folder;
    let icon = match (row.is_folder, row.expanded) {
        (true, true) => IconName::FolderOpen,
        (true, false) => IconName::Folder,
        // 会话记录目前只有 SSH 一种（见 `dialog/connection.rs`）。
        (false, _) => IconName::Globe,
    };
    // caret 只在**有子项的**文件夹上画（空文件夹点了也不会有反应）。
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
        // 行点击：**双击会话行**才按记录开终端（列表是记录，终端是实例）；
        // 其余情况（单击 / 文件夹行双击）= 选中，文件夹顺带展开 / 收起。
        .on_click({
            let state = state.clone();
            move |event, window, cx| {
                if event.click_count() >= 2 && !is_folder {
                    window.dispatch_action(Box::new(OpenSession { path: click_path.clone() }), cx);
                    return;
                }
                let _ = state.update(cx, |sessions, cx| {
                    sessions.activate_row(click_path.clone(), is_folder, cx)
                });
            }
        })
        // 右键菜单（⚠️ 必须在链尾：它会把元素包进一个相对定位的包装元素）。
        // 文件夹行 / 会话行各一套；「删除…」只删列表条目，不会动已开的终端。
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
                .menu("删除文件夹", Box::new(RemoveEntry { path: menu_path.clone() }))
            } else {
                menu.menu(
                    "打开会话",
                    Box::new(OpenSession {
                        path: menu_path.clone(),
                    }),
                )
                .separator()
                .menu("删除会话", Box::new(RemoveEntry { path: menu_path.clone() }))
            }
        })
        .into_any_element()
}
