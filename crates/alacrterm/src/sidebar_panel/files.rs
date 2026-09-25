//! 「文件管理器」视图:远端会话的目录树(走 SFTP,见 `ssh::SshFs`)。
//!
//! **只供远端(SSH)会话使用**(本地目录用系统的文件管理器就好):数据源是当前会话的 SFTP
//! 句柄(每帧由 [`crate::AppRoot`] 同步进来),起始根目录 = 远端家目录。远端 shell 的 `cd`
//! 拿不到,所以根目录**不跟随终端**,由顶部那条路径输入框跳过去([`FilesState::navigate_to`];
//! 根目录变了会写回输入框,[`FilesState::sync_path_input`])。目录**按需异步加载**一层,
//! 排序由 `ssh::SshFs` 负责(目录在前);视图**只读**。
//!
//! ⚠️ **虚拟化靠外层侧边栏的列表**:把树**摊平成一行一项**([`FilesItem`])交给侧边栏自己的
//! 虚拟列表 —— 嵌套第二个虚拟列表会因为内层拿到的可用高度等于整棵树而失效(机制见
//! `docs/terminal-architecture.md` §3.2)。摊平、行骨架与缓存都在 [`super::shared`]。

use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, AsyncApp, Context, CursorStyle, Entity, IntoElement,
    ParentElement as _, Pixels, SharedString, Styled as _, Subscription, WeakEntity, Window, div,
    px,
};
use gpui_kit::component::{
    ActiveTheme as _, h_flex,
    input::{Input, InputEvent, InputState},
    list::ListItem,
    v_flex,
};
use terminal::{RemoteEntry, SshFs};

use super::empty_state;
use super::shared::{RowCache, ellipsis_label, row_content, row_shell};
use crate::assets::IconName;

/// 顶部路径输入框那一行的高度(输入框 @ `Size::Medium` = 32px,上下各留 2px)。
///
/// ⚠️ 不能复用顶部标签条的 [`TAB_HEIGHT`](super::TAB_HEIGHT)(24px):装不下 32px 的输入框,
/// 超出部分会**溢到下面的树上**(不会被裁掉)。
const PATH_ROW_HEIGHT: Pixels = px(36.);

/// 文件树里一个条目的路径:从根目录开始的下标链(`[]` = 根目录本身)。
type FilePath = Vec<usize>;

/// 远端路径拼接(远端是 POSIX 风格,不要用 `PathBuf`)。
fn join_dir(dir: &str, name: &str) -> String {
    format!("{}/{}", dir.trim_end_matches('/'), name)
}

// ---------------------------------------------------------------- 目录读取

/// 文件树里的一个节点。
struct FileNode {
    /// 显示名(文件 / 目录名,不是完整路径)。
    name: SharedString,
    /// 完整远端路径(POSIX):展开这个目录时要用它去读子项。
    path: String,
    /// 是不是目录。
    is_dir: bool,
    /// 读到的子项(`None` = 还没读过这个目录,展开时才读)。
    children: Option<Vec<FileNode>>,
}

/// 把 SFTP 列出的一层目录转成节点(`parent` = 父目录路径)。
fn nodes_from(parent: &str, entries: Vec<RemoteEntry>) -> Vec<FileNode> {
    entries
        .into_iter()
        .map(|entry| FileNode {
            path: join_dir(parent, &entry.name),
            name: entry.name.into(),
            is_dir: entry.is_dir,
            children: None,
        })
        .collect()
}

/// 后台读取的结果回填到哪里。
enum LoadTarget {
    /// 重读**当前**根目录(会话切换 / 切回已知目录):根路径不变。
    Root,
    /// 输入框跳转:列表读成功后再把根目录切成它 —— 读失败就别动,否则面板会闪成空的。
    Navigate(String),
    /// 某个刚被展开的目录。
    Dir(FilePath),
}

// ---------------------------------------------------------------- 视图状态

/// 摊平后的一行(交给侧边栏的虚拟列表)。
#[derive(Clone)]
struct FileRow {
    /// 从根目录开始的下标链:既定位节点,也是展开状态的键。
    path: FilePath,
    label: SharedString,
    /// 缩进层级(根目录下的条目是 0)。
    depth: usize,
    kind: RowKind,
}

/// 这一行是什么(决定图标与能不能展开)。
#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    /// 目录;`expanded` = 当前是否展开。
    Dir { expanded: bool },
    File,
    /// 未加载目录的占位行(「加载中…」)。
    Loading,
}

/// 「文件管理器」视图的状态:远端句柄 + 根目录 + 节点树 + 展开状态 + 摊平后的行清单。
///
/// **数据与交互状态都在这里**:[`crate::AppRoot`] 建好实体后交给两条侧边栏
/// (共用同一个),视图只在远端会话下摆出来;换会话走 [`FilesState::sync`]。
/// 渲染不在这里 —— [`FilesState::sidebar_items`] 把「要摆哪些项」交给侧边栏(它负责虚拟化)。
pub(crate) struct FilesState {
    /// 当前会话的 SFTP 句柄;`None` = 没有远端会话(本地会话 / 没有会话)。
    fs: Option<SshFs>,
    /// 正在显示的根目录(远端 POSIX 绝对路径);`None` = 还没拿到(刚连上,正在问家目录)。
    root: Option<String>,
    /// 根目录下的条目(`None` = 还没读到)。
    entries: Option<Vec<FileNode>>,
    /// 读目录失败的原因(连不上 SFTP / 路径不存在等),显示在空占位上。
    error: Option<String>,
    /// 展开着的目录(下标链)。
    expanded: Vec<FilePath>,
    /// 正在后台读取的目录(同一个目录不重复发起)。
    loading: Vec<String>,
    /// 最近点中的那一行(只做高亮;文件树是只读视图)。
    selected: Option<FilePath>,
    /// 顶部路径输入框:显示 [`FilesState::root`],改内容就跳过去。
    path_input: Entity<InputState>,
    /// 路径还没写回输入框(根目录可能是后台异步问回来的,那时没有窗口 ⇒ 见 [`FilesState::sync`])。
    path_input_pending: bool,
    /// 最近一次路径跳转请求的序号:输入框每敲一个字符就发一次 ⇒ 同时在飞的请求有好几个,
    /// 只有**最新**那次的回答算数(见 [`FilesState::navigate_to`])。
    nav_seq: u64,
    /// 根目录代次:换目录时 +1,在途的读取对不上就丢掉。
    generation: u64,
    /// 摊平后的行清单缓存(见 [`RowCache`]:侧边栏每帧都会来要,不缓存就是每帧 `O(总节点数)`)。
    rows: RowCache<FileRow>,
    /// 路径输入框的事件订阅(RAII:不存着就会在 `new` 返回时解除)。
    _input_sub: Subscription,
}

impl FilesState {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let path_input = cx.new(|cx| InputState::new(window, cx).placeholder("输入远端目录路径"));
        // 用 `subscribe_in`:处理时要 `&mut Window`(写回输入框内容)。
        let input_sub = cx.subscribe_in(
            &path_input,
            window,
            |state: &mut Self, _, event, window, cx| state.on_path_input_event(event, window, cx),
        );
        Self {
            fs: None,
            root: None,
            entries: None,
            error: None,
            expanded: Vec::new(),
            loading: Vec::new(),
            selected: None,
            path_input,
            path_input_pending: false,
            nav_seq: 0,
            generation: 0,
            rows: RowCache::default(),
            _input_sub: input_sub,
        }
    }

    /// 与当前会话对齐:句柄变了(换会话 / 刚连上)就回到远端家目录重新开始,句柄没了就清空。
    ///
    /// 由 [`crate::AppRoot`] 每帧调用(和「文件管理器只服务远端会话」那条判断同一处),
    /// 句柄的比较是 `Arc::ptr_eq`(`SshFs: PartialEq`)⇒ 每帧调用的代价可以忽略。
    pub(crate) fn sync(&mut self, fs: Option<SshFs>, window: &mut Window, cx: &mut Context<Self>) {
        // ⚠️ 写回输入框需要窗口,而根目录可能是后台异步问回来的(见 [`Self::reset_no_window`])
        // ⇒ 在这里补上。不能每帧无条件写:用户可能正在改那个框,会把他的输入顶掉。
        if self.path_input_pending {
            self.path_input_pending = false;
            self.sync_path_input(window, cx);
        }
        if self.fs == fs {
            return;
        }
        self.fs = fs;
        // 新会话的目录还没问过 ⇒ 回 `None`,让 `reset` 去取家目录。
        self.root = None;
        self.reset(window, cx);
    }

    /// 输入框改动 / 回车 ⇒ 尝试跳到里面的路径(回车让「同一个值也想再跳一次」也成立)。
    ///
    /// `_window`:签名要满足 `cx.subscribe_in`(它要求回调收 `&mut Window`),但这里不再需要窗口。
    fn on_path_input_event(
        &mut self,
        event: &InputEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(event, InputEvent::Change | InputEvent::PressEnter { .. }) {
            return;
        }
        let text = self.path_input.read(cx).value().trim().to_string();
        self.navigate_to(&text, cx);
    }

    /// 跳到 `text` 指的目录:**远端确实存在且是目录**才跳,否则什么都不动(空内容同理)。
    ///
    /// 「不存在就不动」是刻意的:打字中途必然经过一串不成立的中间态(把 `/a/b` 改成 `/a/c`
    /// 要先经过 `/a/c` 之前的 `/a/` 那种半截值),那时清空树只会让面板闪成空的。
    ///
    /// ⚠️ 输入框每敲一个字符就发一次,所以同时有多个请求在飞 —— 只有**最新**那次的回答算数
    /// ([`FilesState::nav_seq`]),否则先发的短路径后回来会把树拽回上级目录。
    fn navigate_to(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(fs) = self.fs.clone() else {
            return;
        };
        let text = text.to_string();
        if text.is_empty() || self.root.as_deref() == Some(text.as_str()) {
            return;
        }
        let generation = self.generation;
        self.nav_seq += 1;
        let seq = self.nav_seq;
        // 「是不是目录」要问远端 ⇒ 先发请求,回来再决定跳不跳。
        let task = cx.background_spawn({
            let fs = fs.clone();
            let text = text.clone();
            async move { fs.is_dir(text).await }
        });
        cx.spawn(move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                let is_dir = task.await.unwrap_or(false);
                if !is_dir {
                    return;
                }
                let _ = this.update(&mut cx, |this, cx| {
                    // 期间换过会话 / 换过目录,或者这次已经不是最新请求 ⇒ 这次跳转作废。
                    if this.generation != generation || this.nav_seq != seq {
                        return;
                    }
                    this.generation += 1;
                    this.spawn_load(fs, text.clone(), LoadTarget::Navigate(text), cx);
                });
            }
        })
        .detach();
    }

    /// 把当前根目录写回输入框(换根目录时都走这里)。
    ///
    /// ⚠️ 用 `InputState::set_value`:它内部关掉事件发射 ⇒ 不会回环触发
    /// [`FilesState::on_path_input_event`]。内容已一致时直接返回,免得把光标拽到末尾。
    fn sync_path_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.root.clone().unwrap_or_default();
        // ⚠️ 先把 `read` 的借用收进一条语句(它返回 `Ref` 守卫,留在 `if` 里会一直持有),
        // 否则下面 `update` 会撞成重入借用。
        let current = self.path_input.read(cx).value();
        if current.as_ref() == text {
            return;
        }
        self.path_input
            .update(cx, |input, cx| input.set_value(text, window, cx));
    }

    /// 清空整棵树并重新读根目录(换目录 / 换会话时用)。
    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_path_input(window, cx);
        self.reset_no_window(cx);
    }

    /// [`FilesState::reset`] 里不需要窗口的那半:**所有异步回调**(问家目录 / 路径跳转回来)
    /// 都走它 —— 那时没有窗口,写回输入框留给下一帧(见 [`FilesState::sync`])。
    fn reset_no_window(&mut self, cx: &mut Context<Self>) {
        // 根目录变了 ⇒ 下一帧把新路径写回输入框(那时才有窗口,根目录可能是刚问到的)。
        self.path_input_pending = true;
        // 代次 +1 ⇒ 在途的读取回来会被丢掉,不会把旧目录的内容填进新树。
        self.generation += 1;
        self.entries = None;
        self.error = None;
        self.expanded.clear();
        self.loading.clear();
        self.selected = None;
        // 树 / 展开状态都清了 ⇒ 行清单重建。
        self.rows.bump();
        match (self.fs.clone(), self.root.clone()) {
            // 已知根目录:直接读它。
            (Some(fs), Some(root)) => self.spawn_load(fs, root, LoadTarget::Root, cx),
            // 刚连上会话:先问远端家目录(拿到后再读,见 `spawn_home`)。
            (Some(fs), None) => self.spawn_home(fs, cx),
            // 没有远端会话:清空即可(视图此时也不会摆出来)。
            (None, _) => {}
        }
        cx.notify();
    }

    /// 问远端家目录,拿它当起始根目录。
    fn spawn_home(&mut self, fs: SshFs, cx: &mut Context<Self>) {
        let generation = self.generation;
        let task = cx.background_spawn(async move { fs.home_dir().await });
        cx.spawn(move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                let home = task.await;
                let _ = this.update(&mut cx, |this, cx| {
                    if this.generation != generation {
                        return;
                    }
                    match home {
                        Ok(home) => {
                            this.root = Some(home);
                            this.reset_no_window(cx);
                        }
                        // 连不上 SFTP(认证被拒 / 服务端没开 sftp 子系统)⇒ 说明白原因。
                        Err(error) => {
                            this.error = Some(error.to_string());
                            cx.notify();
                        }
                    }
                });
            }
        })
        .detach();
    }

    /// 后台读一个目录(一层),读完回填(根目录 / 某个刚展开的目录);回填前核对代次。
    fn spawn_load(&mut self, fs: SshFs, dir: String, target: LoadTarget, cx: &mut Context<Self>) {
        if self.loading.contains(&dir) {
            return;
        }
        self.loading.push(dir.clone());
        let generation = self.generation;
        let task = cx.background_spawn({
            let dir = dir.clone();
            async move { fs.list_dir(dir).await }
        });
        cx.spawn({
            let dir = dir.clone();
            move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                // 必须先 clone 再进 async 块(否则借用 `cx` 的生命周期过不了)。
                let mut cx = cx.clone();
                async move {
                    let result = task.await;
                    let _ = this.update(&mut cx, |this, cx| {
                        this.loading.retain(|pending| pending != &dir);
                        if this.generation != generation {
                            return;
                        }
                        // 内容变了 ⇒ 行清单要重建。
                        this.rows.bump();
                        let entries = match result {
                            Ok(entries) => {
                                this.error = None;
                                nodes_from(&dir, entries)
                            }
                            // 读不到(没有这个路径 / 没权限 / 连接断了):空目录 + 一句原因。
                            Err(error) => {
                                this.error = Some(error.to_string());
                                Vec::new()
                            }
                        };
                        match &target {
                            // 重读当前根目录:根路径不变。
                            LoadTarget::Root => this.entries = Some(entries),
                            // 跳转成功:到这里才换根目录(失败时旧树原样留着)。
                            LoadTarget::Navigate(root) => {
                                this.root = Some(root.clone());
                                this.path_input_pending = true;
                                this.expanded.clear();
                                this.entries = Some(entries);
                            }
                            LoadTarget::Dir(path) => {
                                if let Some(entries_root) = this.entries.as_mut()
                                    && let Some(node) = Self::node_mut(entries_root, path)
                                {
                                    node.children = Some(entries);
                                }
                            }
                        }
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 摊平后的行清单(见 [`RowCache`];侧边栏每帧都会来要一次)。
    fn rows(&mut self) -> Rc<Vec<FileRow>> {
        if let Some(rows) = self.rows.cached() {
            return rows;
        }
        let mut out = Vec::new();
        let mut path = FilePath::new();
        Self::flatten(
            self.entries.as_deref().unwrap_or(&[]),
            &self.expanded,
            &mut path,
            &mut out,
        );
        self.rows.store(out)
    }

    /// [`FilesState::rows`] 的递归实现:按展开状态把节点树摊成一行行。
    fn flatten(
        nodes: &[FileNode],
        expanded: &[FilePath],
        path: &mut FilePath,
        out: &mut Vec<FileRow>,
    ) {
        for (ix, node) in nodes.iter().enumerate() {
            path.push(ix);
            let kind = if node.is_dir {
                RowKind::Dir {
                    expanded: expanded.contains(path),
                }
            } else {
                RowKind::File
            };
            out.push(FileRow {
                path: path.clone(),
                label: node.name.clone(),
                depth: path.len() - 1,
                kind,
            });
            // 展开的目录要接着摊子项;还没读过的目录摊一行「加载中…」占位。
            if let RowKind::Dir { expanded: true } = kind {
                match &node.children {
                    Some(children) => Self::flatten(children, expanded, path, out),
                    None => out.push(FileRow {
                        path: path.clone(),
                        label: "加载中…".into(),
                        depth: path.len(),
                        kind: RowKind::Loading,
                    }),
                }
            }
            path.pop();
        }
    }

    /// 按行号取一行(行数刚变过时外层可能还在渲染旧下标 ⇒ 取不到就返 `None`)。
    fn row(&self, ix: usize) -> Option<FileRow> {
        self.rows.row(ix)
    }

    /// 交给侧边栏虚拟列表的全部内容项:路径输入框 + 每一行(或一句空占位)。
    pub(super) fn sidebar_items(&mut self, cx: &mut Context<Self>) -> Vec<FilesItem> {
        // 远端目录信息还没到(含首层列表在途,否则会先闪一句「这个目录是空的」)⇒ 什么都不摆。
        if self.error.is_none() && (self.root.is_none() || self.entries.is_none()) {
            return Vec::new();
        }

        // 问家目录失败:连根目录都没有 ⇒ 只摆原因,路径输入框也不摆(空框没东西可跳)。
        if self.root.is_none() {
            return vec![FilesItem::Placeholder {
                icon: IconName::FolderClosed,
                title: "无法读取远端目录",
                description: "当前会话的远端文件系统不可用",
                detail: self.error.clone().map(SharedString::from),
            }];
        }

        let rows = self.rows();
        let mut items = Vec::with_capacity(rows.len() + 2);
        // 顶部路径输入框(跟着内容一起滚动,与改造前一致)。
        items.push(FilesItem::Path(self.path_input.clone()));
        if rows.is_empty() {
            items.push(FilesItem::Placeholder {
                icon: IconName::FolderOpen,
                title: "这个目录是空的",
                description: "可以用上面的路径框跳到别的目录",
                detail: self.error.clone().map(SharedString::from),
            });
        } else {
            let state = cx.entity().downgrade();
            items.extend((0..rows.len()).map(|ix| FilesItem::Row {
                state: state.clone(),
                ix,
            }));
        }
        items
    }

    /// 点一行:目录展开 / 收起(未读过的目录顺带发起加载),文件行只记选中。
    fn activate_row(&mut self, path: FilePath, kind: RowKind, cx: &mut Context<Self>) {
        self.selected = Some(path.clone());
        if matches!(kind, RowKind::Dir { .. }) {
            let was_open = self.expanded.iter().any(|open| open == &path);
            self.expanded.retain(|open| open != &path);
            if !was_open {
                self.expanded.push(path.clone());
                // 目录还没读过 ⇒ 读它(读完后回填子项、行清单再重建)。
                let pending = self
                    .node(&path)
                    .filter(|node| node.is_dir && node.children.is_none())
                    .map(|node| node.path.clone());
                if let (Some(dir), Some(fs)) = (pending, self.fs.clone()) {
                    self.spawn_load(fs, dir, LoadTarget::Dir(path), cx);
                }
            }
            self.rows.bump();
        }
        cx.notify();
    }

    /// 按路径取节点(`[]` 取不到 ⇒ `None`)。
    fn node(&self, path: &[usize]) -> Option<&FileNode> {
        let mut nodes: &[FileNode] = self.entries.as_deref()?;
        let (last, parent) = path.split_last()?;
        for step in parent {
            nodes = nodes.get(*step)?.children.as_deref()?;
        }
        nodes.get(*last)
    }

    /// 按路径取**可变**节点(回填子项时用;递归实现,迭代版过不了借用检查)。
    fn node_mut<'a>(nodes: &'a mut Vec<FileNode>, path: &[usize]) -> Option<&'a mut FileNode> {
        let (ix, rest) = path.split_first()?;
        if rest.is_empty() {
            return nodes.get_mut(*ix);
        }
        let node = nodes.get_mut(*ix)?;
        Self::node_mut(node.children.as_mut()?, rest)
    }
}

// ---------------------------------------------------------------- 侧边栏内容项

/// 文件管理器交给侧边栏虚拟列表的一项。
///
/// ⚠️ **一行一项**:侧边栏内容区是它自己的虚拟列表,只渲染可见区间 + overdraw;
/// 再嵌一个内层虚拟列表的话,内层拿到的可用高度会等于整棵树 ⇒ 虚拟化失效(见模块文档)。
#[derive(Clone)]
pub(super) enum FilesItem {
    /// 顶部路径输入框。
    Path(Entity<InputState>),
    /// 文件树的一行(`ix` 是行清单下标,渲染时按需从 [`FilesState`] 读)。
    Row {
        state: WeakEntity<FilesState>,
        ix: usize,
    },
    /// 空占位(没有可浏览的目录 / 目录是空的 / 读目录失败)。
    Placeholder {
        icon: IconName,
        title: &'static str,
        description: &'static str,
        /// 失败原因(有则补一行小字)。
        detail: Option<SharedString>,
    },
}

impl FilesItem {
    /// 画这一项。`cx` 是侧边栏的 `&mut App`(读行数据、接点击都在这里)。
    pub(super) fn render(self, cx: &mut App) -> AnyElement {
        match self {
            // 路径输入框整行铺满(32px 的输入框 + 上下各 2px,见 [`PATH_ROW_HEIGHT`])。
            Self::Path(input) => h_flex()
                .w_full()
                .h(PATH_ROW_HEIGHT)
                .px_2()
                .child(
                    div().flex_1().min_w_0().child(
                        Input::new(&input)
                            // 不要清除按钮(路径跟着会话走,一键清空只会把面板弄空)。
                            .cleanable(false)
                            .aria_label("远端目录路径"),
                    ),
                )
                .into_any_element(),
            Self::Row { state, ix } => {
                // 行数刚变过时外层可能还在渲染旧下标 ⇒ 取不到就摆个空元素。
                let Ok(Some(row)) = state.read_with(&*cx, |files, _| files.row(ix)) else {
                    return div().w_full().into_any_element();
                };
                let selected = state
                    .read_with(&*cx, |files, _| {
                        files.selected.as_deref() == Some(row.path.as_slice())
                    })
                    .unwrap_or(false);
                let path = row.path.clone();
                let kind = row.kind;
                let element = row_element(ix, &row, selected, cx);
                // 占位行不接受点击;目录点一下展开 / 收起,文件行只记选中。
                let element = if matches!(kind, RowKind::Loading) {
                    element.into_any_element()
                } else {
                    element
                        .on_click(move |_, _window, cx| {
                            let _ = state
                                .update(cx, |files, cx| files.activate_row(path.clone(), kind, cx));
                        })
                        .into_any_element()
                };
                div().w_full().child(element).into_any_element()
            }
            Self::Placeholder {
                icon,
                title,
                description,
                detail,
            } => {
                let mut body = v_flex()
                    .w_full()
                    .gap_2()
                    .child(empty_state(icon, title, Some(description)));
                if let Some(detail) = detail {
                    body = body.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(detail),
                    );
                }
                body.into_any_element()
            }
        }
    }
}

// ---------------------------------------------------------------- 行的渲染

/// 文件树的一行:目录(可展开)/ 文件 / 占位(「加载中…」)。
///
/// caret 只在目录行上画(未加载的目录也有 —— 行清单里它后面跟着一行占位)。
fn row_element(ix: usize, row: &FileRow, selected: bool, cx: &mut App) -> ListItem {
    let label = row.label.clone();
    let item = row_shell(ix, row.depth, selected, cx).cursor(match row.kind {
        RowKind::Dir { .. } => CursorStyle::PointingHand,
        _ => CursorStyle::Arrow,
    });
    match row.kind {
        RowKind::Dir { expanded } => item.child(row_content(
            Some(if expanded {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            }),
            if expanded {
                IconName::FolderOpen
            } else {
                IconName::Folder
            },
            label,
        )),
        RowKind::File => item.child(row_content(None, IconName::File, label)),
        // 占位行(「加载中…」):不该有任何交互,退化成一行灰字。
        RowKind::Loading => item.child(ellipsis_label(label)),
    }
}
