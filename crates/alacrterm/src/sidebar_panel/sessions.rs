//! 「会话」视图:文件夹形式的**记录树**(MobaXterm 那种形态),状态收在 [`SessionsState`]。
//!
//! 列表内容是**会话记录**(连接配置),与终端实例**完全无关**:一条记录有没有在跑终端,
//! 取决于用户是否双击过它;关掉终端也不影响列表。**没有自动生成的根文件夹**,顶层
//! 直接就是用户自己建的文件夹(可嵌套,行上不显示「里面有几条」)与记录。
//!
//! 本模块 = 记录模型 + 「会话」视图的状态与全部增删改,都在 [`SessionsState`] 这个实体里
//! (由 [`crate::AppRoot`] 持有,见 [`crate::main`] 的模块文档):
//! - **加**:状态栏左下角建文件夹([`NewFolder`])、右下角新建会话([`NewSession`]),
//!   都落顶层;文件夹行右键可以「在这里新建会话 / 新建子文件夹」(落进那个文件夹)。
//! - **删**:记录行 / 文件夹行的右键菜单;文件夹连带里面的内容一起删。
//! - **开终端**:双击记录行(单击只选中)或右键「打开会话」→ 派发 [`OpenSession`],
//!   由 [`crate::AppRoot::open_session_record`] 取记录再建终端(跨组件那一步留在根视图)。
//! - **拖放**:记录行与文件夹行都能拖(载荷 [`DragSessionEntry`]);落在文件夹行 = 进它、
//!   落在会话行 = 进它所在目录、落在树下方那条空白落点 = 提到顶层;落点合法性由
//!   [`SessionsState::move_entry`] 把关(不能拖进自己 / 自己的子孙)。
//!
//! ⚠️ 几个 gpui-kit `tree` 的坑(踩过):
//! - **行类型由行 id 前缀判断**([`row_id`] / [`session_row`]),**不能**用
//!   `TreeEntry::is_folder()` —— 它其实是「有没有子项」,空文件夹会被当成叶子;
//! - `Tree` 是**等高虚拟列表 + `size_full()`**,塞进 `Sidebar` 的自动高度 item 里会塌成 0
//!   ⇒ 必须 `.h(可见行数 × TREE_ROW_HEIGHT)`,行数由 [`SessionsState::visible_rows`] 算;
//! - **展开状态不能存在 `TreeItem` 里**(每次同步都会重建 `TreeItem`)⇒ 存在
//!   [`SessionsState::expanded`](按路径),用户展收靠订阅 [`TreeEvent`] 回写;
//! - `set_items` 会 notify ⇒ **不能每帧无条件调用**(靠内容签名挡,否则自激成死循环),
//!   且签名要用 `Option`(空 `Vec` 会让首次同步被当成「签名没变」跳掉、列表一片空白)。
//!
//! 行交互一律**派发 action**([`OpenSession`] / [`RemoveEntry`] / …):`SidebarItem::render`
//! 只拿得到 `&mut App`,而 action 走的是与应用其它入口完全相同的路径(见 [`crate::actions`])。

use gpui::{
    AnyElement, App, AppContext as _, Context, CursorStyle, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_kit::component::{
    ActiveTheme as _, Icon, StyledExt as _,
    h_flex,
    list::ListItem,
    tree::{TreeEntry, TreeEvent, TreeItem, TreeState, tree},
    v_flex,
};

use super::TAB_HEIGHT;
use crate::actions::{MoveEntry, NewFolder, NewSession, OpenSession, RemoveEntry};
use crate::assets::IconName;
use crate::terminal_panel::SessionRequest;
use util::shell::Shell;

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

    /// 这一项是文件夹吗（⚠️ 不能用 gpui-kit 的 `TreeItem::is_folder()` 代替：
    /// 它按「有没有子项」判断，空文件夹会被当成叶子）。
    pub(crate) fn is_folder(&self) -> bool {
        matches!(self, Self::Folder(_))
    }
}

/// 会话列表里的一个文件夹。
pub(crate) struct SessionFolder {
    /// 文件夹名（行标题）。
    pub(crate) name: SharedString,
    /// 子条目（文件夹 / 记录混排，顺序即显示顺序）。
    pub(crate) children: Vec<SessionEntry>,
}

impl SessionFolder {
    pub(crate) fn new(name: SharedString) -> Self {
        Self {
            name,
            children: Vec::new(),
        }
    }
}

/// 会话列表里的一条**会话记录**：只保存连接参数，不保存任何运行状态。
///
/// 同一条记录可以开任意多个终端（双击记录），关掉终端也不会影响记录本身。
/// 只在内存里，重启不保留。
pub(crate) struct SessionRecord {
    /// 显示名（列表里的行标题，也是打开后标签页的名字）。
    pub(crate) name: SharedString,
    /// 登录用户名。
    pub(crate) user: String,
    /// 主机地址（IP / 域名）。
    pub(crate) host: String,
    /// SSH 端口。
    pub(crate) port: String,
}

impl SessionRecord {
    /// 打开这条记录要用的会话参数（见 [`crate::terminal_panel::SessionRequest`]）。
    pub(crate) fn request(&self) -> SessionRequest {
        SessionRequest {
            name: Some(self.name.clone()),
            shell: Shell::WithArguments {
                program: "ssh".to_string(),
                args: vec![
                    "-p".to_string(),
                    self.port.clone(),
                    format!("{}@{}", self.user, self.host),
                ],
                // 显示名由回传的 `name` 统一管理，这里不再重复指定。
                title_override: None,
            },
            target: crate::terminal_panel::SessionTarget::Ssh {
                user: self.user.clone(),
                host: self.host.clone(),
                port: self.port.clone(),
            },
        }
    }
}

// ---------------------------------------------------------------- 视图状态

/// 会话树的每行高度:`Tree` 内部的虚拟列表是**等高**的,所以这个值也得用来算整棵树的高度。
const TREE_ROW_HEIGHT: Pixels = px(28.);

/// 「会话」视图（记录树）的状态：记录模型 + 展开状态 + gpui-kit [`TreeState`]。
///
/// 由 [`crate::AppRoot`] 持有一个 `Entity<SessionsState>`：**列表的数据、树的交互状态、
/// 以及对它们的全部操作、连同渲染都在这里**，根视图完全不插手（侧边栏只把这个实体
/// 摆进内容位，见 `sidebar_panel::SidebarContent`）。
pub(crate) struct SessionsState {
    /// 侧边栏「会话」列表的条目树（顺序 = 列表里的显示顺序）。
    ///
    /// 只有「新建会话 / 新建文件夹」对话框与 action 监听器会往这里加条目；
    /// 条目与 [`crate::AppRoot::terminals`] **没有对应关系**（双击记录才按它开一个终端）。
    entries: Vec<SessionEntry>,
    /// 展开着的文件夹路径。
    ///
    /// 会话树的展开状态**本可以**存在 `TreeItem` 内部（共享的 `Rc<RefCell<..>>`），
    /// 但条目树每次同步都会重建 `TreeItem` ⇒ 那样会每刷一次就全部收起。
    /// 所以这里自己留一份：同步时按它给 `TreeItem::expanded(..)` 赋值，
    /// 同时它也是[【算行数】](SessionsState::visible_rows)的依据
    /// （树的高度必须手算，见本实体自己的 [`Render`] 实现）。
    expanded: Vec<SessionPath>,
    /// 「会话」树的交互状态（选中 / 滚动 / 键盘导航）。
    tree: Entity<TreeState>,
    /// 上一次同步树用的内容签名（条目类型 + 名字 + 层级，含被收起的分支）：
    /// 变了才重建 items（`set_items` 会 notify，每帧无条件调用会自激）。
    ///
    /// ⚠️ `None` = 还没同步过：不能用「空 `Vec`」同时表达「没同步过」与「一条都没有」——
    /// 后者会让首次同步被当成「签名没变」跳掉，树就永远拿不到 items。
    tree_sig: Option<Vec<(SharedString, bool, usize)>>,
    /// 树的展收事件订阅（RAII：不存着就会在 `new` 返回时解除）。
    _tree_sub: Subscription,
}

impl SessionsState {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let tree = cx.new(|cx| TreeState::new(cx));
        // 文件夹展开 / 收起会改变「树一共多少行」（高度按行数算），
        // 所以树的事件要立刻回传到本实体。
        let sub = cx.subscribe(&tree, |state: &mut Self, _, event, cx| {
            state.on_tree_event(event, cx);
        });
        Self {
            entries: Vec::new(),
            expanded: Vec::new(),
            tree,
            tree_sig: None,
            _tree_sub: sub,
        }
    }

    /// 会话树当前显示多少行（展开的文件夹才计入子项）。
    ///
    /// ⚠️ `Tree` 内部是**虚拟列表 + `size_full()`**，而它是塞在 `Sidebar` 自己的虚拟列表里的
    /// 一个自动高度 item ⇒ 拿不到确定高度、高度会塌成 0。所以必须自己把行数算出来
    /// （见本实体自己的 [`Render`] 实现）。
    fn visible_rows(&self) -> usize {
        let mut path = SessionPath::new();
        Self::count_rows(&self.entries, &self.expanded, &mut path)
    }

    /// [`SessionsState::visible_rows`] 的递归实现（`path` 是当前递归位置）。
    fn count_rows(entries: &[SessionEntry], expanded: &[SessionPath], path: &mut SessionPath) -> usize {
        let mut rows = 0;
        for (ix, entry) in entries.iter().enumerate() {
            rows += 1;
            path.push(ix);
            if let SessionEntry::Folder(folder) = entry
                && expanded.iter().any(|open| open == path)
            {
                rows += Self::count_rows(&folder.children, expanded, path);
            }
            path.pop();
        }
        rows
    }

    /// 把会话列表（文件夹 + 记录）同步进 `TreeState`（在 `AppRoot::render` 开头调用）。
    ///
    /// 两个要点：
    /// 1. `TreeState::set_items` 会 `notify` ⇒ **不能每帧无条件调用**，这里靠内容签名
    ///    （条目的「类型 + 名字 + 层级」全量序列，含被收起的子树）挡一下，否则自激成死循环；
    /// 2. 每次同步都会**重建** `TreeItem`，所以展开状态不能存在树里：它存在
    ///    [`SessionsState::expanded`]（用户展开 / 收起时由 [`SessionsState::on_tree_event`]
    ///    更新），同步时按它给每个文件夹行 `.expanded(..)` 赋值。
    ///
    /// 列表内容是**记录**，与终端实例无关：所以这里不问 `AppRoot::terminals`，
    /// 也不去同步「当前会话」的高亮——选中项就是用户点过的那一行，由树自己管。
    pub(crate) fn sync_tree(&mut self, cx: &mut Context<Self>) {
        let mut sig = Vec::new();
        Self::collect_sig(&self.entries, 0, &mut sig);
        if self.tree_sig.as_ref() == Some(&sig) {
            return;
        }
        self.tree_sig = Some(sig);

        let mut path = SessionPath::new();
        let items = Self::build_items(&self.entries, &self.expanded, &mut path);
        self.tree
            .update(cx, |state, cx| state.set_items(items, cx));
    }

    /// 内容签名的一段（递归收集）：`(名字, 是不是文件夹, 层级)`。
    ///
    /// 带上层级是为了让「把条目挪进 / 挪出文件夹」这种结构变化也能被感知到
    /// （光看名字序列，`[文件夹 A, 记录 x]` 与 `[文件夹 A[记录 x]]` 长得一样）。
    fn collect_sig(
        entries: &[SessionEntry],
        depth: usize,
        out: &mut Vec<(SharedString, bool, usize)>,
    ) {
        for entry in entries {
            out.push((entry.label(), entry.is_folder(), depth));
            if let SessionEntry::Folder(folder) = entry {
                Self::collect_sig(&folder.children, depth + 1, out);
            }
        }
    }

    /// 按列表内容建树项（`path` 是当前递归位置，既是行 id 也是展开状态的键）。
    fn build_items(
        entries: &[SessionEntry],
        expanded: &[SessionPath],
        path: &mut SessionPath,
    ) -> Vec<TreeItem> {
        let mut items = Vec::with_capacity(entries.len());
        for (ix, entry) in entries.iter().enumerate() {
            path.push(ix);
            let mut item = TreeItem::new(row_id(entry.is_folder(), path), entry.label());
            if let SessionEntry::Folder(folder) = entry {
                item = item.expanded(expanded.iter().any(|open| open == path));
                item.children = Self::build_items(&folder.children, expanded, path);
            }
            path.pop();
            items.push(item);
        }
        items
    }

    /// 树事件：用户展开 / 收起文件夹 ⇒ 更新 [`SessionsState::expanded`] 并重绘。
    ///
    /// 树的高度是按行数算出来的，所以展收必须回传到本实体（不能等别的重绘顺手带上）。
    fn on_tree_event(&mut self, event: &TreeEvent, cx: &mut Context<Self>) {
        let (id, expand) = match event {
            TreeEvent::Expanded(id) => (id, true),
            TreeEvent::Collapsed(id) => (id, false),
        };
        let Some(path) = path_of_id(id.as_ref()) else {
            return;
        };
        self.expanded.retain(|open| open != &path);
        if expand {
            self.expanded.push(path);
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
        record: SessionRecord,
        folder: Option<SessionPath>,
        cx: &mut Context<Self>,
    ) {
        let target = folder.unwrap_or_default();
        if let Some(children) = Self::children_mut(&mut self.entries, &target) {
            children.push(SessionEntry::Session(record));
            // 落进文件夹时顺手展开它，否则新条目看不见。
            self.expand_folder(&target);
        }
        cx.notify();
    }

    /// 新建一个文件夹（`parent` = 建在哪个文件夹下，`None` = 顶层）。
    pub(crate) fn add_folder(
        &mut self,
        name: SharedString,
        parent: Option<SessionPath>,
        cx: &mut Context<Self>,
    ) {
        let target = parent.unwrap_or_default();
        if let Some(children) = Self::children_mut(&mut self.entries, &target) {
            children.push(SessionEntry::Folder(SessionFolder::new(name)));
            self.expand_folder(&target);
        }
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
            children.remove(*ix);
            // 删掉一项后，同一父目录下排在它后面的条目下标都会前移一位，
            // 展开标记（按路径存）要跟着挪，否则展开状态会串到邻居身上。
            Self::shift_paths_after_removal(&mut self.expanded, path);
        }
        cx.notify();
    }

    /// 把一个条目挪到 `into` 这个文件夹下（拖放放下）：`into` 为空 = 顶层。
    ///
    /// 追加到目标目录末尾（不做行间插入——用户要的是「拖到别的文件夹下」）；
    /// 同时把被拖动那棵子树的展开标记一起搬到新位置。
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

        // 跟随被拖动子树一起搬走的展开标记（存的是相对于 `from` 的相对路径）。
        let mut carried: Vec<SessionPath> = Vec::new();
        self.expanded.retain(|open| {
            if open.starts_with(from) {
                carried.push(open[from.len()..].to_vec());
                false
            } else {
                true
            }
        });
        // 取出来之后，同一父目录下排在后面的展开标记下标要前移一位。
        Self::shift_paths_after_removal(&mut self.expanded, from);
        // 目标路径也得按「取出后」的下标算（同一个目录里时会差一位）。
        let removed = Self::shift_path_after_removal(into.to_vec(), from);

        let entry = Self::take_entry(&mut self.entries, from).expect("刚刚确认过这条路径有条目");
        let destination = Self::children_mut(&mut self.entries, &removed)
            .expect("目标文件夹刚刚校验过");
        let landed = destination.len();
        destination.push(entry);

        // 子树里原来展开的文件夹，在新位置继续展开；目标目录本身也展开，
        // 否则刚拖进去的条目看不见。
        for relative in carried {
            let mut open = removed.clone();
            open.push(landed);
            open.extend(relative);
            self.expanded.push(open);
        }
        self.expand_folder(&removed);
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

    /// 把一个文件夹标记为展开（新条目落进它时用；它同时决定树的行数）。
    fn expand_folder(&mut self, path: &[usize]) {
        if !self.expanded.iter().any(|open| open == path) {
            self.expanded.push(path.to_vec());
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

    /// [`SessionsState::shift_path_after_removal`] 的批量版（用于展开标记列表）。
    fn shift_paths_after_removal(paths: &mut [SessionPath], removed: &[usize]) {
        for path in paths {
            let shifted = Self::shift_path_after_removal(std::mem::take(path), removed);
            *path = shifted;
        }
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

/// 「会话」视图的**渲染**：gpui-kit 的 [`tree`]（文件夹形式的会话树）+ 下方一条
/// 「拖到顶层」的空白落点。
///
/// 数据与交互状态都在本实体里（条目树 / 展开状态 / `Entity<TreeState>`），所以渲染也放在这里：
/// 侧边栏只把这个实体摆进它的内容位（见 `sidebar_panel::SidebarContent`）。
///
/// 行的交互一律**派发 action**（[`OpenSession`] / [`RemoveEntry`] / …）而不是直接改状态：
/// `ListItem` 的回调只拿得到 `&mut App`，而 action 走的是与应用其它入口完全相同的路径
/// （见 [`crate::actions`]）。
///
/// ⚠️ `Tree` 内部是**虚拟列表 + `size_full()`**，而它是塞在 `Sidebar` 自己的虚拟列表里的
/// 一个自动高度 item ⇒ 拿不到确定高度、高度会塌成 0。所以必须
/// `.h(行数 × TREE_ROW_HEIGHT)` 手动给高度（`Tree` 的 `refine_style` 在链尾，能盖住
/// `size_full`）；行数由 [`SessionsState::visible_rows`] 按展开状态算。
impl Render for SessionsState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.visible_rows();
        // 一条条目都没有时给一句提示：树本身零行、什么也不画，而列表是空的看不出「能点什么」。
        if rows == 0 {
            return div()
                .px_3()
                .py_2()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(
                    "还没有会话：左下角 + 建文件夹，右下角 + 新建会话；双击会话条目打开终端",
                )
                .into_any_element();
        }

        // 树 + 下方一条**拖到顶层**的空白落点：条目拖出文件夹后要有地方可放
        // （见 [`drop_into`]）。它同时也让最后一行下面留出一点呼吸空间。
        v_flex()
            .w_full()
            .child(
                tree(&self.tree, move |ix, entry, selected, _window, cx| {
                    session_tree_row(ix, entry, selected, cx)
                })
                .h(TREE_ROW_HEIGHT * rows as f32)
                // 右键菜单由树统一挂（文件夹行 / 会话行各一套）：
                // 「打开会话」按记录开终端，「删除…」只删列表条目，都不会动已开的终端。
                .context_menu(move |_ix, entry, menu, _, _| match session_row(entry) {
                    Some(SessionRow::Session(path)) => menu
                        .menu("打开会话", Box::new(OpenSession { path: path.clone() }))
                        .separator()
                        .menu("删除会话", Box::new(RemoveEntry { path })),
                    Some(SessionRow::Folder(path)) => menu
                        .menu(
                            "在这里新建会话",
                            Box::new(NewSession {
                                folder: Some(path.clone()),
                            }),
                        )
                        .menu("新建子文件夹", Box::new(NewFolder { parent: Some(path.clone()) }))
                        .separator()
                        .menu("删除文件夹", Box::new(RemoveEntry { path })),
                    None => menu,
                }),
            )
            .child(
                div()
                    .id("session-tree-top-level-drop")
                    .w_full()
                    .h(px(16.))
                    .drag_over::<DragSessionEntry>(|style, _, _, cx| {
                        style.bg(cx.theme().tokens.accent)
                    })
                    .on_drop(move |drag: &DragSessionEntry, window, cx| {
                        drop_into(drag.path.clone(), None, window, cx)
                    }),
            )
            .into_any_element()
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

/// 拖会话条目时跟着鼠标的小卡片（形状同侧边栏标签拖拽的预览）。
struct SessionDragPreview {
    label: SharedString,
}

impl Render for SessionDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .h(TAB_HEIGHT)
            .flex()
            .items_center()
            .rounded(cx.theme().radius)
            .bg(cx.theme().tokens.accent)
            .text_color(cx.theme().accent_foreground)
            .child(self.label.clone())
    }
}

/// 放下：把条目挪进 `into` 这个文件夹（`None` = 顶层），动作交给
/// [`MoveEntry`](crate::actions::MoveEntry) 统一处理（合法性校验也在那边）。
fn drop_into(from: SessionPath, into: Option<SessionPath>, window: &mut Window, cx: &mut App) {
    window.dispatch_action(Box::new(MoveEntry { from, into }), cx);
}

/// 会话树里一行指向什么（由行 id 解出，见 [`row_id`]）。
///
/// ⚠️ 不能用 `TreeEntry::is_folder()` 判断类型：gpui-kit 的 `TreeItem::is_folder()`
/// 是「**有没有子项**」的意思，空文件夹会被当成叶子（于是画成会话行、双击还想开终端）。
/// 所以类型编码在行 id 的前缀里。
#[derive(Clone, PartialEq, Eq)]
enum SessionRow {
    Folder(SessionPath),
    Session(SessionPath),
}

/// 行 id：`folder-0-2` = 顶层第 0 个文件夹里的第 2 个条目；`session-1` = 顶层第 1 个条目。
///
/// 前缀区分类型（见 [`SessionRow`] 里的说明），后面用 `-` 串起路径下标——
/// 与 `SessionsState` 的模型路径（[`SessionPath`]）一一对应，增删 / 嵌套都不会错位。
fn row_id(is_folder: bool, path: &[usize]) -> SharedString {
    let mut id = String::from(if is_folder { "folder" } else { "session" });
    for ix in path {
        id.push('-');
        id.push_str(&ix.to_string());
    }
    id.into()
}

/// 从行 id 解出类型与路径（`folder-0-2` / `session-1`）。
fn path_of_id(id: &str) -> Option<SessionPath> {
    let (_, rest) = id.split_once('-')?;
    rest.split('-').map(|step| step.parse().ok()).collect()
}

/// 行 id → [`SessionRow`]（前缀无法识别时 `None`，此时按「不可交互」处理）。
fn session_row(entry: &TreeEntry) -> Option<SessionRow> {
    let id = entry.item().id.as_ref();
    let (kind, _) = id.split_once('-')?;
    let path = path_of_id(id)?;
    match kind {
        "folder" => Some(SessionRow::Folder(path)),
        "session" => Some(SessionRow::Session(path)),
        _ => None,
    }
}

/// 会话树的一行（文件夹行 / 会话行）。
///
/// **同一目录下的文件夹与会话是同级**：两行都从 `pl(层级缩进)` 开始，会话行不再额外
/// 缩进一个 caret 的宽度（否则会话看上去像比同目录的文件夹低一级）。
/// - **文件夹行**：子项非空时给 caret（展开 / 收起由 `TreeState`
///   自己的行点击处理），否则只画文件夹图标（空文件夹点了也不会有反应）；
/// - **会话行**：**单击只选中，双击才建终端**
///   （`OpenSession` → [`crate::AppRoot::open_session_record`]）
///   ——列表是记录，终端是实例，两者刻意分开。
///
/// **两行都可以拖动**（载荷 [`DragSessionEntry`]）：拖到文件夹行上 = 放进那个文件夹，
/// 拖到会话行上 = 放进它所在的那个目录，拖到列表下方的空白条 = 提到顶层；
/// 落点合法性（不能拖进自己 / 自己的子孙）由 [`SessionsState::move_entry`] 把关。
///
/// 两行的悬停 / 选中样式由 gpui-kit 的 `ListItem` 统一画（选中底色 = 主题 `accent`，
/// 见 [`crate::config::change_theme`] 里关掉 `list.active_highlight` 的原因）。
fn session_tree_row(ix: usize, entry: &TreeEntry, selected: bool, cx: &mut App) -> ListItem {
    let (radius, accent_fg) = {
        let theme = cx.theme();
        (theme.radius, theme.sidebar_accent_foreground)
    };
    let label = entry.item().label.clone();
    let depth = entry.depth() as f32;
    // 缩进：每层 16px，顶层留 8px 内边距；文件夹与会话共用。
    let indent = px(8. + depth * 16.);
    let row = ListItem::new(ix)
        .h(TREE_ROW_HEIGHT)
        .pr_2()
        .rounded(radius)
        .text_sm()
        .overflow_x_hidden()
        .cursor(CursorStyle::PointingHand)
        .when(selected, |this| this.font_medium().text_color(accent_fg));

    match session_row(entry) {
        // 文件夹行：拖到它上面 = 放进它里面。
        Some(SessionRow::Folder(path)) => {
            let payload = DragSessionEntry {
                path: path.clone(),
                label: label.clone(),
            };
            let drop_path = path.clone();
            row.pl(indent)
                .on_drag(payload, move |drag: &DragSessionEntry, _, _, cx| {
                    cx.new(|_| SessionDragPreview {
                        label: drag.label.clone(),
                    })
                })
                .drag_over::<DragSessionEntry>(|style, _, _, cx| {
                    style.bg(cx.theme().tokens.accent)
                })
                .on_drop(move |drag: &DragSessionEntry, window, cx| {
                    drop_into(drag.path.clone(), Some(drop_path.clone()), window, cx)
                })
                .child(
                    h_flex()
                        .gap_x_2()
                        .items_center()
                        // caret 只在「真的有子项」时画（`is_folder()` = 有没有子项）。
                        .when(entry.is_folder(), |this| {
                            this.child(Icon::new(if entry.is_expanded() {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .size_3())
                        })
                        .child(Icon::new(if entry.is_expanded() {
                            IconName::FolderOpen
                        } else {
                            IconName::Folder
                        })
                        .size_3())
                        .child(ellipsis_label(label)),
                )
        }
        // 会话行：拖到它上面 = 放进**它所在的目录**（同级末尾），
        // 这样「拖到另一个文件夹里的某个会话上」也会进那个文件夹。
        Some(SessionRow::Session(path)) => {
            let payload = DragSessionEntry {
                path: path.clone(),
                label: label.clone(),
            };
            let drop_path = path.clone();
            let open_path = path;
            row
                // 与同目录的文件夹行同一缩进（同级），不再多缩一个 caret 宽度。
                .pl(indent)
                // 双击（`click_count >= 2`）才按记录开终端；单击留给树自己选中。
                .on_click(move |event, window, cx| {
                    if event.click_count() < 2 {
                        return;
                    }
                    window.dispatch_action(Box::new(OpenSession { path: open_path.clone() }), cx);
                })
                .on_drag(payload, move |drag: &DragSessionEntry, _, _, cx| {
                    cx.new(|_| SessionDragPreview {
                        label: drag.label.clone(),
                    })
                })
                .drag_over::<DragSessionEntry>(|style, _, _, cx| {
                    style.bg(cx.theme().tokens.accent)
                })
                .on_drop(move |drag: &DragSessionEntry, window, cx| {
                    // 目标的父目录（`session-0` 的父目录就是顶层）。
                    let parent = drop_path[..drop_path.len() - 1].to_vec();
                    drop_into(drag.path.clone(), Some(parent), window, cx)
                })
                .child(
                    h_flex()
                        .gap_x_2()
                        .items_center()
                        // 会话记录目前只有 SSH 一种（见 `dialog/connection.rs`）。
                        .child(Icon::new(IconName::Globe).size_3())
                        .child(ellipsis_label(label)),
                )
        }
        // id 不是我们的两种前缀：不该出现，退化成一行纯文字（不挂任何交互）。
        None => row.pl(indent).child(label),
    }
}

/// 行标题：单行省略号（树不换行，窄侧边栏里长名字要被截断）。
fn ellipsis_label(label: SharedString) -> AnyElement {
    div()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .child(label)
        .into_any_element()
}
