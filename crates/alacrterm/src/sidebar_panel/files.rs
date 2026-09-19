//! 「文件管理器」视图:当前终端工作目录下的文件树。
//!
//! 根目录 = **当前会话**的工作目录([`TerminalView::working_directory`],由根视图每帧
//! 同步进来)。⚠️ 拿得到的是**进程真实的工作目录**,所以只有真正切换工作目录的壳
//! (cmd / bash / wsl / 自己 `chdir` 的程序)才会跟随;PowerShell 的 `cd` 只改它自己的
//! 位置(`$PWD`)、不动进程的当前目录,因此 PowerShell 下树会停在会话启动目录
//! (Zed / Windows Terminal 在 Windows 上也是这个行为)。远端(SSH)会话拿不到**远端**
//! shell 的 cwd,此时显示一句提示。
//!
//! 顶部那行显示当前根目录,并有两枚按钮:**上一级**(手动往上浏览整块磁盘,终端工作目录
//! 再变时自动拉回)与**重新加载**(目录内容变了,而树不监听文件系统)。
//!
//! **目录按需加载**:展开一个还没读过的目录时,后台线程读一层目录项再回填
//! (见 [`FilesState::spawn_load`]),网络盘 / 超大目录不会卡住界面。读取结果按
//! 「目录在前、名字不区分大小写」排序。视图是**只读**的:行点击只会展开 / 收起,
//! 没有打开文件、重命名之类的操作;顶部那行显示当前路径并提供「重新加载」按钮。
//!
//! ⚠️ gpui-kit `tree` 的坑(与 [`sessions`](super::sessions) 同一批,踩过):
//! - **行类型编码在行 id 前缀里**(`dir-0-2` / `file-1` / `loading-0-2`),
//!   不能用 `TreeEntry::is_folder()` —— 它是「有没有子项」;
//! - `Tree` 是**等高虚拟列表 + `size_full()`**,塞进 `Sidebar` 的自动高度 item 里会塌成 0
//!   ⇒ 必须 `.h(可见行数 × TREE_ROW_HEIGHT)`,行数由 [`FilesState::visible_rows`] 手算;
//! - **未加载的目录必须挂一个占位子项**(label 为「加载中…」):`TreeItem::is_folder()` 就是
//!   「有没有子项」,没有子项的行既没有 caret、点击也不会展开([`TreeState::toggle_expand`]
//!   对非 folder 直接 return)⇒ 子目录就永远打不开;
//! - 展开状态自己存([`FilesState::expanded`]):每次同步都会**重建** `TreeItem`,
//!   状态存在它里面会每刷一次就全部收起;
//! - `set_items` 会 notify ⇒ **不能每帧无条件调用**(靠内容签名挡,否则自激成死循环),
//!   签名必须用 `Option` 表达「还没同步过」(用空 `Vec` 会让首次同步被当成「没变」跳掉)。

use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, App, AppContext as _, AsyncApp, Context, CursorStyle, Entity, IntoElement,
    ParentElement as _, Pixels, Render, SharedString, Styled as _, Subscription, WeakEntity, Window,
    div, prelude::FluentBuilder as _, px,
};
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    list::ListItem,
    tree::{TreeEntry, TreeEvent, TreeItem, TreeState, tree},
    v_flex,
};

use super::TAB_HEIGHT;
use crate::assets::IconName;

/// 文件树每行高度:`Tree` 内部的虚拟列表是**等高**的,所以这个值也得用来算整棵树的高度。
const TREE_ROW_HEIGHT: Pixels = px(28.);

/// 文件树里一个条目的路径:从根目录开始的下标链(`[]` = 根目录本身)。
type FilePath = Vec<usize>;

// ---------------------------------------------------------------- 目录读取

/// 文件树里的一个节点。
struct FileNode {
    /// 显示名(文件 / 目录名,不是完整路径)。
    name: SharedString,
    /// 完整路径:展开这个目录时要用它去读子项。
    path: PathBuf,
    /// 是不是目录。
    is_dir: bool,
    /// 读到的子项(`None` = 还没读过这个目录,展开时才读)。
    children: Option<Vec<FileNode>>,
}

/// 读一层目录,按「目录在前、名字不区分大小写」排序(资源管理器的习惯)。
///
/// 读不到(不存在 / 没权限)时返回空列表:文件树里表现为「这个目录是空的」,
/// 不额外弹错(终端里本来就能看到真正的报错)。
fn read_dir_sorted(dir: &Path) -> Vec<FileNode> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut nodes = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| FileNode {
            name: entry.file_name().to_string_lossy().into_owned().into(),
            is_dir: entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false),
            path: entry.path(),
            children: None,
        })
        .collect::<Vec<_>>();
    nodes.sort_by_cached_key(|node| (!node.is_dir, node.name.to_lowercase()));
    nodes
}

/// 后台读取的结果回填到哪里。
enum LoadTarget {
    /// 换根目录(终端 `cd` / 刷新按钮)。
    Root,
    /// 某个刚被展开的目录。
    Dir(FilePath),
}

// ---------------------------------------------------------------- 视图状态

/// 「文件管理器」视图的状态:当前根目录 + 已读到的节点树 + 展开状态 + gpui-kit [`TreeState`]。
///
/// 由 [`crate::AppRoot`] 持有一个 `Entity<FilesState>`(两条侧边栏共用):**数据、树的交互
/// 状态、以及渲染都在这里**,根视图只负责每帧把当前会话的工作目录同步进来
/// ([`FilesState::sync`]),侧边栏只把这个实体摆进内容位。
pub(crate) struct FilesState {
    /// 当前会话的工作目录（终端侧的真值）：只有它变了才把根目录拉回去。
    ///
    /// `None` = 没有会话，或远端会话拿不到本地路径。
    cwd: Option<PathBuf>,
    /// 正在显示的根目录：默认就是 [`FilesState::cwd`]；用户点了「上一级」之后会停在父目录，
    /// 直到终端的工作目录再次变化（那时又拉回终端所在目录）。
    root: Option<PathBuf>,
    /// 根目录下的条目(`None` = 还没读到)。
    entries: Option<Vec<FileNode>>,
    /// 展开着的目录路径(下标链)。
    ///
    /// 与 `sessions` 同一个理由:**不能**把展开状态留在 `TreeItem` 里 ——
    /// 每次同步都会重建 `TreeItem`,状态会每刷一次就丢。这里自己留一份:同步时按它给
    /// `TreeItem::expanded(..)` 赋值,同时它也是[【算行数】](FilesState::visible_rows)的依据。
    expanded: Vec<FilePath>,
    /// 正在后台读取的目录(同一个目录不重复发起)。
    loading: Vec<PathBuf>,
    /// 根目录代次:换目录时 +1,在途的读取回来发现对不上就丢掉(否则旧目录的结果会覆盖新目录)。
    generation: u64,
    /// 文件树的交互状态(选中 / 滚动)。
    tree: Entity<TreeState>,
    /// 上一次同步用的签名(`根目录 + (名字, 是否目录, 层级)` 全量序列):
    /// 变了才重建 items(`set_items` 会 notify,每帧无条件调用会自激)。
    ///
    /// ⚠️ `None` = 还没同步过:不能用一个空 `Vec` 同时表达「没同步过」与「根目录是空的」,
    /// 后者会让首次同步被当成「签名没变」跳掉,树就永远拿不到 items。
    sig: Option<(Option<PathBuf>, Vec<(SharedString, bool, usize)>)>,
    /// 树的展收事件订阅(RAII:不存着就会在 `new` 返回时解除)。
    _tree_sub: Subscription,
}

impl FilesState {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let tree = cx.new(|cx| TreeState::new(cx));
        // 展开 / 收起会改变「树一共多少行」(高度按行数算),所以树的事件要立刻回传到本实体;
        // 展开一个还没读过的目录时还要顺手发起后台读取。
        let sub = cx.subscribe(&tree, |state: &mut Self, _, event, cx| {
            state.on_tree_event(event, cx);
        });
        Self {
            cwd: None,
            root: None,
            entries: None,
            expanded: Vec::new(),
            loading: Vec::new(),
            generation: 0,
            tree,
            sig: None,
            _tree_sub: sub,
        }
    }

    /// 与当前会话的工作目录对齐(根视图每帧调用,`cwd` 是当前会话的进程工作目录)。
    ///
    /// 只在**目录真的变了**时动手:换根目录会清空整棵树并重新读一层,同时让在途的读取作废。
    /// 用户手动「上一级」后停在外面的目录,也会在终端换位置时被拉回终端所在目录。
    pub(crate) fn sync(&mut self, cwd: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.cwd == cwd {
            return;
        }
        self.cwd = cwd.clone();
        self.root = cwd;
        self.reset(cx);
    }

    /// 上一级(顶部那枚向上按钮):手动往上浏览,不回改终端的工作目录。
    fn go_up(&mut self, cx: &mut Context<Self>) {
        let Some(parent) = self.root.as_ref().and_then(|root| root.parent()) else {
            return;
        };
        self.root = Some(parent.to_path_buf());
        self.reset(cx);
    }

    /// 重新读当前根目录(顶部那枚刷新按钮):目录内容会变(新建 / 删除文件),而树本身不监听文件系统。
    fn reload(&mut self, cx: &mut Context<Self>) {
        if self.root.is_none() {
            return;
        }
        self.reset(cx);
    }

    /// 清空整棵树并重新读根目录(换目录 / 刷新共用)。
    fn reset(&mut self, cx: &mut Context<Self>) {
        // 代次 +1 ⇒ 在途的读取回来会被丢掉,不会把旧目录的内容填进新树。
        self.generation += 1;
        self.entries = None;
        self.expanded.clear();
        self.sig = None;
        if let Some(dir) = self.root.clone() {
            self.spawn_load(dir, LoadTarget::Root, cx);
        }
        // 立刻把(暂时为空的)树推给 `TreeState`,免得旧目录的行残留到新目录读回来为止。
        self.sync_tree(cx);
        cx.notify();
    }

    /// 后台读一个目录,读完回填(根目录 / 某个刚展开的目录)。
    ///
    /// 用 `background_spawn`(后台线程池)读 **一层** 目录项:目录可能很大、也可能在慢盘上,
    /// 放主线程会把界面卡住;回填时先核对代次,换过目录的结果直接丢。
    fn spawn_load(&mut self, dir: PathBuf, target: LoadTarget, cx: &mut Context<Self>) {
        if self.loading.contains(&dir) {
            return;
        }
        self.loading.push(dir.clone());
        let generation = self.generation;
        let task = cx.background_spawn({
            let dir = dir.clone();
            async move { read_dir_sorted(&dir) }
        });
        cx.spawn({
            let dir = dir.clone();
            move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                // ⚠️ 必须在闭包内先 clone 再进 async 块,否则借用 `cx` 的 lifetime 过不了。
                let mut cx = cx.clone();
                async move {
                    let nodes = task.await;
                    let _ = this.update(&mut cx, |this, cx| {
                        this.loading.retain(|pending| pending != &dir);
                        if this.generation != generation {
                            return;
                        }
                        // 内容变了 ⇒ 作废签名,让 `sync_tree` 重建。
                        this.sig = None;
                        match &target {
                            LoadTarget::Root => this.entries = Some(nodes),
                            LoadTarget::Dir(path) => {
                                if let Some(entries) = this.entries.as_mut()
                                    && let Some(node) = Self::node_mut(entries, path)
                                {
                                    node.children = Some(nodes);
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

    /// 把节点树推给 `TreeState`(内容没变就跳过,见 [`FilesState::sig`])。
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
    /// 带上层级,这样「文件挪进 / 挪出子目录」这类结构变化也能被感知
    /// (光看名字序列,`[目录 A, 文件 x]` 与 `[目录 A[文件 x]]` 长得一样)。
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
                    // 还没读过这个目录:挂一个占位子项。**必须挂** ——
                    // `TreeItem::is_folder()` 是「有没有子项」,没有子项的行既没有 caret,
                    // 点击也不会展开(`TreeState::toggle_expand` 对非 folder 直接 return)。
                    None => vec![TreeItem::new(placeholder_id(path), "加载中…")],
                };
            }
            path.pop();
            items.push(item);
        }
        items
    }

    /// 树事件:用户展开 / 收起一行 ⇒ 更新 [`FilesState::expanded`],需要时后台读该目录。
    ///
    /// 树的高度是按行数算出来的,所以展收必须回传到本实体(不能等别的重绘顺手带上)。
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
            if let Some(dir) = pending {
                self.spawn_load(dir, LoadTarget::Dir(path), cx);
            }
        }
        cx.notify();
    }

    /// 树当前显示多少行(展开的目录才计入子项)。
    ///
    /// ⚠️ `Tree` 内部是**虚拟列表 + `size_full()`**,塞在 `Sidebar` 自己的虚拟列表里拿不到
    /// 确定高度 ⇒ 必须自己算出来并 `.h(行数 × TREE_ROW_HEIGHT)`。算法要与
    /// [`FilesState::build_items`] + `TreeState::add_entry` 的展平规则**完全一致**
    /// (包括「未加载目录挂一个占位子项」那条)。
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.visible_rows();
        // 顶部一行显示当前根目录(长路径省略号截断),右端是「上一级」与「重新加载」。
        let header = h_flex()
            .w_full()
            .h(TAB_HEIGHT)
            .px_2()
            .gap_1()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(ellipsis_label(
                self.root
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "当前会话没有可浏览的目录".to_string())
                    .into(),
            ))
            .child(
                div().flex_none().child(
                    Button::new("files-up")
                        .ghost()
                        .xsmall()
                        .icon(IconName::ArrowUp)
                        .tooltip("上一级")
                        .on_click(cx.listener(|this, _, _, cx| this.go_up(cx))),
                ),
            )
            .child(
                div().flex_none().child(
                    Button::new("files-reload")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Replace)
                        .tooltip("重新加载当前目录")
                        .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                ),
            );

        let body: AnyElement = match (self.root.is_some(), rows) {
            // 没有会话 / 远端会话:拿不到本地目录(远端 shell 的 cwd 探测不到,见模块文档)。
            (false, _) => hint("远端会话或无会话时显示不了文件", cx),
            (true, 0) => hint("这个目录是空的", cx),
            (true, _) => tree(&self.tree, |ix, entry, selected, _window, cx| {
                file_tree_row(ix, entry, selected, cx)
            })
            .h(TREE_ROW_HEIGHT * rows as f32)
            .into_any_element(),
        };

        v_flex().w_full().child(header).child(body).into_any_element()
    }
}

// ---------------------------------------------------------------- 行的渲染

/// 文件树的一行:目录(可展开)/ 文件 / 占位(「加载中…」)。
///
/// 行的展开 / 收起由 `TreeState` 自己那套行点击处理(`on_entry_click` → `toggle_expand`),
/// 我们只负责画:caret 只在「真的有子项」时画 —— 未加载的目录挂着占位子项,所以也有 caret。
/// 文件行没有任何交互(视图是只读的)。
///
/// 悬停 / 选中样式由 gpui-kit 的 `ListItem` 统一画(选中底色 = 主题 `accent`,
/// 见 [`crate::config::change_theme`] 里关掉 `list.active_highlight` 的原因)。
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
/// ⚠️ **不能**用 `TreeEntry::is_folder()`:它是「有没有子项」的意思,未加载的目录也挂着
/// 占位子项,空目录反而没有子项 —— 用它判类型会把两者都判反。
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

/// 一句灰字提示(空目录 / 拿不到目录),免得面板看上去是坏的。
fn hint(text: &'static str, cx: &App) -> AnyElement {
    div()
        .px_3()
        .py_2()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text)
        .into_any_element()
}
