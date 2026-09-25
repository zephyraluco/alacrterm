//! 「文件管理器」视图:远端会话的目录树(走 SFTP,见 `ssh::SshFs`)。
//!
//! **只供远端(SSH)会话使用**:本地目录用系统自己的文件管理器打开就好,应用里再摆一份没有
//! 意义,所以本地终端下这个视图连同标签一起不摆(见 [`super::SidebarView::visible_with`])。
//! 数据源是当前会话的 SFTP 句柄(每帧由 [`crate::AppRoot`] 同步进来),起始根目录 = 远端家目录;
//! 远端 shell 的 `cd` 拿不到(要改远端 prompt 才行),所以根目录**不跟随终端**,由用户自己跳。
//!
//! 顶部是一条**路径输入框**,与「显示中的根目录」双向对齐:改内容就尝试跳过去(只认确实是
//! 目录的路径,[`FilesState::navigate_to`]);根目录变了则写回输入框
//! ([`FilesState::sync_path_input`])。没有目录可显示时连这条也不摆,只剩空占位。
//!
//! **目录按需异步加载**:展开未读过的目录时后台读一层再回填,排序由 `ssh::SshFs` 负责
//! (目录在前、名字不区分大小写)。视图**只读**(行点击只展开 / 收起)。
//!
//! ⚠️ gpui-kit `tree` 的约定(与 [`sessions`](super::sessions) 同一批):
//! - 行类型编码在行 id 前缀里(`dir-0-2` / `file-1` / `loading-0-2`),不用 `TreeEntry::is_folder()`;
//! - `Tree` 是等高虚拟列表 + `size_full()` ⇒ 必须自己 `.h(可见行数 × TREE_ROW_HEIGHT)`;
//! - **未加载的目录要挂占位子项**(「加载中…」),否则没 caret、点了也不展开;
//! - 展开状态自己存(`FilesState::expanded`);`set_items` 会 notify ⇒ 靠签名挡。

use gpui::{
    AnyElement, App, AppContext as _, AsyncApp, Context, CursorStyle, Entity, IntoElement,
    ParentElement as _, Pixels, Render, SharedString, Styled as _, Subscription, WeakEntity, Window,
    div, prelude::FluentBuilder as _, px,
};
use gpui_kit::component::{
    ActiveTheme as _, Icon, StyledExt as _,
    h_flex,
    input::{Input, InputEvent, InputState},
    list::ListItem,
    tree::{TreeEntry, TreeEvent, TreeItem, TreeState, tree},
    v_flex,
};
use terminal::{RemoteEntry, SshFs};

use super::empty_state;
use crate::assets::IconName;

/// 文件树每行高度:`Tree` 内部的虚拟列表是**等高**的,所以这个值也得用来算整棵树的高度。
const TREE_ROW_HEIGHT: Pixels = px(28.);

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
    /// 换根目录(首次连上 / 输入框跳转)。
    Root,
    /// 某个刚被展开的目录。
    Dir(FilePath),
}

// ---------------------------------------------------------------- 视图状态

/// 「文件管理器」视图的状态:远端句柄 + 根目录 + 节点树 + 展开状态 + gpui-kit [`TreeState`]。
///
/// **数据、树的交互状态与渲染都在这里**:[`crate::AppRoot`] 建好实体后交给两条侧边栏
/// (共用同一个),视图只在远端会话下摆出来;换会话走 [`FilesState::sync`]。
pub(crate) struct FilesState {
    /// 当前会话的 SFTP 句柄;`None` = 没有远端会话(本地会话 / 没有会话)。
    fs: Option<SshFs>,
    /// 正在显示的根目录(远端 POSIX 绝对路径);`None` = 还没拿到(刚连上,正在问家目录)。
    root: Option<String>,
    /// 根目录下的条目(`None` = 还没读到)。
    entries: Option<Vec<FileNode>>,
    /// 读目录失败的原因(连不上 SFTP / 路径不存在等),显示在空占位上。
    error: Option<String>,
    /// 展开着的目录(下标链;同步时给 `TreeItem::expanded(..)` 赋值,也是算行数的依据)。
    expanded: Vec<FilePath>,
    /// 正在后台读取的目录(同一个目录不重复发起)。
    loading: Vec<String>,
    /// 顶部路径输入框:显示 [`FilesState::root`],改内容就跳过去。
    path_input: Entity<InputState>,
    /// 路径还没写回输入框(根目录可能是后台异步问回来的,那时没有窗口 ⇒ 见 [`FilesState::sync`])。
    path_input_pending: bool,
    /// 最近一次路径跳转请求的序号:输入框每敲一个字符就发一次 ⇒ 同时在飞的请求有好几个,
    /// 只有**最新**那次的回答算数(见 [`FilesState::navigate_to`])。
    nav_seq: u64,
    /// 根目录代次:换目录时 +1,在途的读取对不上就丢掉。
    generation: u64,
    /// 文件树的交互状态(选中 / 滚动)。
    tree: Entity<TreeState>,
    /// 上一次同步用的签名(`根目录 + (名字, 是否目录, 层级)` 全量序列):变了才重建 items。
    /// ⚠️ `None` = 还没同步过(用空 `Vec` 表示会让首次同步被当成「没变」跳掉)。
    sig: Option<(Option<String>, Vec<(SharedString, bool, usize)>)>,
    /// 树的展收事件订阅(RAII:不存着就会在 `new` 返回时解除)。
    _tree_sub: Subscription,
    /// 路径输入框的事件订阅(同上)。
    _input_sub: Subscription,
}

impl FilesState {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tree = cx.new(|cx| TreeState::new(cx));
        // 展收会改行数(高度按行数算),且展开未读过的目录要发起后台读取 ⇒ 事件回传本实体。
        let sub = cx.subscribe(&tree, |state: &mut Self, _, event, cx| {
            state.on_tree_event(event, cx);
        });
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
            path_input,
            path_input_pending: false,
            nav_seq: 0,
            generation: 0,
            tree,
            sig: None,
            _tree_sub: sub,
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
                    this.root = Some(text);
                    this.reset_no_window(cx);
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
        self.sig = None;
        match (self.fs.clone(), self.root.clone()) {
            // 已知根目录:直接读它。
            (Some(fs), Some(root)) => self.spawn_load(fs, root, LoadTarget::Root, cx),
            // 刚连上会话:先问远端家目录(拿到后再读,见 `spawn_home`)。
            (Some(fs), None) => self.spawn_home(fs, cx),
            // 没有远端会话:清空即可(视图此时也不会摆出来)。
            (None, _) => {}
        }
        // 立刻把(暂时为空的)树推给 `TreeState`,免得旧目录的行残留到新目录读回来为止。
        self.sync_tree(cx);
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
                        // 内容变了 ⇒ 作废签名,让 `sync_tree` 重建。
                        this.sig = None;
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
                            LoadTarget::Root => this.entries = Some(entries),
                            LoadTarget::Dir(path) => {
                                if let Some(entries_root) = this.entries.as_mut()
                                    && let Some(node) = Self::node_mut(entries_root, path)
                                {
                                    node.children = Some(entries);
                                }
                            }
                        }
                        this.sync_tree(cx);
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 把节点树推给 `TreeState`(内容没变就跳过)。
    fn sync_tree(&mut self, cx: &mut Context<Self>) {
        let nodes = self.entries.as_deref().unwrap_or(&[]);
        let mut sig = Vec::new();
        Self::collect_sig(nodes, 0, &mut sig);
        let sig = (self.root.clone(), sig);
        if self.sig.as_ref() == Some(&sig) {
            return;
        }
        self.sig = Some(sig);

        let mut path = FilePath::new();
        let items = Self::build_items(nodes, &self.expanded, &mut path);
        self.tree
            .update(cx, |state, cx| state.set_items(items, cx));
    }

    /// 内容签名的一段(递归):`(名字, 是不是目录, 层级)`。
    ///
    /// 带层级是为了感知「文件挪进 / 挪出子目录」(光看名字序列这两种长得一样)。
    fn collect_sig(
        nodes: &[FileNode],
        depth: usize,
        out: &mut Vec<(SharedString, bool, usize)>,
    ) {
        for node in nodes {
            out.push((node.name.clone(), node.is_dir, depth));
            if let Some(children) = &node.children {
                Self::collect_sig(children, depth + 1, out);
            }
        }
    }

    /// 按节点树建树项(`path` 是当前递归位置:既是行 id,也是展开状态的键)。
    fn build_items(nodes: &[FileNode], expanded: &[FilePath], path: &mut FilePath) -> Vec<TreeItem> {
        let mut items = Vec::with_capacity(nodes.len());
        for (ix, node) in nodes.iter().enumerate() {
            path.push(ix);
            let mut item = TreeItem::new(row_id(node.is_dir, path), node.name.clone())
                .expanded(expanded.contains(path));
            if node.is_dir {
                item.children = match &node.children {
                    Some(children) => Self::build_items(children, expanded, path),
                    // 还没读过:挂一个占位子项。**必须挂** —— 没有子项的行既没 caret、
                    // 点了也不展开(`TreeState::toggle_expand` 对非 folder 直接 return)。
                    None => vec![TreeItem::new(placeholder_id(path), "加载中…")],
                };
            }
            path.pop();
            items.push(item);
        }
        items
    }

    /// 树事件:展开 / 收起一行 ⇒ 更新 [`FilesState::expanded`],需要时后台读该目录。
    ///
    /// 树的高度按行数算,所以展收必须回传本实体(不能等别的重绘顺手带上)。
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
            self.expanded.push(path.clone());
            // 目录还没读过 ⇒ 读它(读完自己会 `sync_tree`;展开状态先记着,回来时按它赋值)。
            let pending = self
                .node(&path)
                .filter(|node| node.is_dir && node.children.is_none())
                .map(|node| node.path.clone());
            if let (Some(dir), Some(fs)) = (pending, self.fs.clone()) {
                self.spawn_load(fs, dir, LoadTarget::Dir(path), cx);
            }
        }
        cx.notify();
    }

    /// 树当前显示多少行(展开的目录才计入子项)。
    ///
    /// ⚠️ `Tree` 是虚拟列表 + `size_full()`,塞在 `Sidebar` 自己的虚拟列表里拿不到确定高度
    /// ⇒ 必须自己算出行数并 `.h(行数 × TREE_ROW_HEIGHT)`;算法要与
    /// [`FilesState::build_items`] + `TreeState::add_entry` 的展平规则**逐条一致**(含占位行)。
    fn visible_rows(&self) -> usize {
        let mut rows = 0;
        let mut path = FilePath::new();
        Self::count_rows(
            self.entries.as_deref().unwrap_or(&[]),
            &self.expanded,
            &mut path,
            &mut rows,
        );
        rows
    }

    /// [`FilesState::visible_rows`] 的递归实现(`path` 是当前递归位置)。
    fn count_rows(
        nodes: &[FileNode],
        expanded: &[FilePath],
        path: &mut FilePath,
        rows: &mut usize,
    ) {
        for (ix, node) in nodes.iter().enumerate() {
            *rows += 1;
            path.push(ix);
            if node.is_dir && expanded.contains(path) {
                match &node.children {
                    Some(children) => Self::count_rows(children, expanded, path, rows),
                    // 未加载的展开目录:树里只有那一个占位行。
                    None => *rows += 1,
                }
            }
            path.pop();
        }
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

impl Render for FilesState {
    /// `_cx`:本实现只画自己的状态,主题色都在行渲染闭包自己那份 `cx` 上取。
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // 还没有根目录(没有远端会话 / 正在问家目录 / 问失败):
        // 只摆空占位,**连顶部那条路径输入框也不摆** —— 一条空框既没内容可编辑、也没东西可跳。
        if self.root.is_none() {
            let placeholder = match &self.error {
                // 连不上 SFTP:把原因说清楚(多半是认证被拒 / 服务端没开 sftp 子系统)。
                Some(error) => v_flex()
                    .gap_2()
                    .child(empty_state(
                        IconName::FolderClosed,
                        "无法读取远端目录",
                        Some("当前会话的远端文件系统不可用"),
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(_cx.theme().muted_foreground)
                            .child(error.clone()),
                    )
                    .into_any_element(),
                None => empty_state(
                    IconName::FolderClosed,
                    "还没有可浏览的目录",
                    Some("打开一个 SSH 会话后，这里显示它的家目录"),
                )
                .into_any_element(),
            };
            return placeholder;
        }

        let rows = self.visible_rows();
        // 顶部一行:路径输入框整行铺满(高 32px > 标签条的 24px ⇒ 用 [`PATH_ROW_HEIGHT`])。
        let header = h_flex()
            .w_full()
            .h(PATH_ROW_HEIGHT)
            .px_2()
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&self.path_input)
                        // 不要清除按钮(路径跟着会话走,一键清空只会把面板弄空)。
                        .cleanable(false)
                        .aria_label("远端目录路径"),
                ),
            );

        // 目录里一条都没有:换成空占位(输入框留着 —— 还能靠它跳去别的目录)。
        let body: AnyElement = if rows == 0 {
            let description = match &self.error {
                Some(error) => error.clone(),
                None => "这个目录是空的".to_string(),
            };
            v_flex()
                .gap_2()
                .child(empty_state(
                    IconName::FolderOpen,
                    "这个目录是空的",
                    None,
                ))
                .child(
                    div()
                        .text_xs()
                        .text_color(_cx.theme().muted_foreground)
                        .child(description),
                )
                .into_any_element()
        } else {
            tree(&self.tree, |ix, entry, selected, _window, cx| {
                file_tree_row(ix, entry, selected, cx)
            })
            .h(TREE_ROW_HEIGHT * rows as f32)
            .into_any_element()
        };

        // 根目录只在上面那条输入框里显示(它随 `root` 变化写回,见 [`FilesState::sync_path_input`])。
        v_flex().w_full().child(header).child(body).into_any_element()
    }
}

// ---------------------------------------------------------------- 行的渲染

/// 文件树的一行:目录(可展开)/ 文件 / 占位(「加载中…」)。
///
/// 展收由 `TreeState` 自己处理(`on_entry_click` → `toggle_expand`),我们只负责画:caret 只在
/// 真的有子项时画(未加载的目录挂着占位子项,所以也有);文件行没有交互(视图只读)。
///
/// 悬停 / 选中样式由 [`ListItem`] 统一画(选中底色 = 主题 `accent`,见
/// [`crate::config::change_theme`] 里关掉 `list.active_highlight` 的原因)。
fn file_tree_row(ix: usize, entry: &TreeEntry, selected: bool, cx: &mut App) -> ListItem {
    let (radius, accent_fg) = {
        let theme = cx.theme();
        (theme.radius, theme.sidebar_accent_foreground)
    };
    let label = entry.item().label.clone();
    let indent = px(8. + entry.depth() as f32 * 16.);
    let row = ListItem::new(ix)
        .h(TREE_ROW_HEIGHT)
        .pr_2()
        .rounded(radius)
        .text_sm()
        .overflow_x_hidden()
        .cursor(if row_is_dir(entry) == Some(true) {
            CursorStyle::PointingHand
        } else {
            CursorStyle::Arrow
        })
        .when(selected, |this| this.font_medium().text_color(accent_fg));

    match row_is_dir(entry) {
        Some(true) => row.pl(indent).child(
            h_flex()
                .gap_x_2()
                .items_center()
                .when(entry.is_folder(), |this| {
                    this.child(
                        Icon::new(if entry.is_expanded() {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .size_3(),
                    )
                })
                .child(
                    Icon::new(if entry.is_expanded() {
                        IconName::FolderOpen
                    } else {
                        IconName::Folder
                    })
                    .size_3(),
                )
                .child(ellipsis_label(label)),
        ),
        Some(false) => row
            .pl(indent)
            .child(
                h_flex()
                    .gap_x_2()
                    .items_center()
                    .child(Icon::new(IconName::File).size_3())
                    .child(ellipsis_label(label)),
            ),
        // 占位行(「加载中…」):不该有任何交互,退化成一行灰字。
        None => row.pl(indent).child(ellipsis_label(label)),
    }
}

/// 这一行是目录 / 文件 / 占位(由行 id 前缀判断,见 [`row_id`])。
///
/// ⚠️ **不能**用 `TreeEntry::is_folder()`:它的意思是「有没有子项」,未加载的目录挂着占位
/// 子项、空目录反而没有 ⇒ 判类型会把两者都判反。
fn row_is_dir(entry: &TreeEntry) -> Option<bool> {
    let id = entry.item().id.as_ref();
    let (kind, _) = id.split_once('-')?;
    match kind {
        "dir" => Some(true),
        "file" => Some(false),
        _ => None,
    }
}

/// 目录 / 文件行的 id(`dir-0-2` / `file-1`),下标链与 [`FilePath`] 一一对应。
fn row_id(is_dir: bool, path: &[usize]) -> SharedString {
    encode_id(if is_dir { "dir" } else { "file" }, path)
}

/// 未加载目录的占位子项 id(前缀 `loading` ⇒ [`row_is_dir`] 返回 `None`,画成灰字)。
fn placeholder_id(path: &[usize]) -> SharedString {
    encode_id("loading", path)
}

/// 拼一个 `前缀-下标-下标` 形式的行 id。
fn encode_id(kind: &str, path: &[usize]) -> SharedString {
    let mut id = String::from(kind);
    for ix in path {
        id.push('-');
        id.push_str(&ix.to_string());
    }
    id.into()
}

/// 从行 id 解出路径(`dir-0-2` / `file-1` / `loading-0-2`)。
fn path_of_id(id: &str) -> Option<FilePath> {
    let (_, rest) = id.split_once('-')?;
    rest.split('-').map(|step| step.parse().ok()).collect()
}

/// 行标题 / 路径:单行省略号(树不换行,窄侧边栏里长名字要被截断)。
fn ellipsis_label(label: SharedString) -> AnyElement {
    div()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .child(label)
        .into_any_element()
}
