//! 「文件管理器」视图:当前终端工作目录下的文件树。
//!
//! 根目录 = **当前会话**的工作目录(Windows 上 PowerShell 的位置由 shell 集成上报,见
//! `terminal::platform`);远端(SSH)会话拿不到远端 cwd,此时摆空占位(见 [`super::empty_state`])。
//!
//! 顶部是一条**路径输入框**,与「显示中的根目录」双向对齐:改内容就尝试跳过去(只认确实是
//! 目录的路径,[`FilesState::navigate_to`]);终端 `cd` 改了目录则写回输入框
//! ([`FilesState::sync_path_input`])。没有目录可显示时(终端全关掉 / 远端会话)连这条也不摆,
//! 只剩空占位(见 [`super::empty_state`])。
//!
//! **目录按需加载**:展开未读过的目录时后台读一层再回填,排序 = 目录在前、名字不区分大小写。
//! 视图**只读**(行点击只展开 / 收起)。
//!
//! ⚠️ gpui-kit `tree` 的约定(与 [`sessions`](super::sessions) 同一批):
//! - 行类型编码在行 id 前缀里(`dir-0-2` / `file-1` / `loading-0-2`),不用 `TreeEntry::is_folder()`;
//! - `Tree` 是等高虚拟列表 + `size_full()` ⇒ 必须自己 `.h(可见行数 × TREE_ROW_HEIGHT)`;
//! - **未加载的目录要挂占位子项**(「加载中…」),否则没 caret、点了也不展开;
//! - 展开状态自己存(`FilesState::expanded`);`set_items` 会 notify ⇒ 靠签名挡。

use std::path::{Path, PathBuf};

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

/// 读一层目录,按「目录在前、名字不区分大小写」排序。
///
/// 读不到(不存在 / 没权限)返回空列表 ⇒ 树里表现为「这个目录是空的」,不额外弹错。
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
    /// 换根目录(终端 `cd` / 输入框跳转)。
    Root,
    /// 某个刚被展开的目录。
    Dir(FilePath),
}

// ---------------------------------------------------------------- 视图状态

/// 「文件管理器」视图的状态:根目录 + 节点树 + 展开状态 + gpui-kit [`TreeState`]。
///
/// **数据、树的交互状态与渲染都在这里**:[`crate::AppRoot`] 持有一个 `Entity<FilesState>`
/// (两条侧边栏共用),每帧把当前会话的工作目录同步进来([`FilesState::sync`])。
pub(crate) struct FilesState {
    /// 当前会话的工作目录(终端侧真值):只有它变了才把根目录拉回去。
    /// `None` = 没有会话,或远端会话拿不到本地路径。
    cwd: Option<PathBuf>,
    /// 正在显示的根目录(默认 = [`FilesState::cwd`]);终端 cwd 变了会被拉回终端所在目录。
    root: Option<PathBuf>,
    /// 根目录下的条目(`None` = 还没读到)。
    entries: Option<Vec<FileNode>>,
    /// 展开着的目录(下标链;同步时给 `TreeItem::expanded(..)` 赋值,也是算行数的依据)。
    expanded: Vec<FilePath>,
    /// 正在后台读取的目录(同一个目录不重复发起)。
    loading: Vec<PathBuf>,
    /// 顶部路径输入框:显示 [`FilesState::root`],改内容就跳过去。
    path_input: Entity<InputState>,
    /// 根目录代次:换目录时 +1,在途的读取对不上就丢掉。
    generation: u64,
    /// 文件树的交互状态(选中 / 滚动)。
    tree: Entity<TreeState>,
    /// 上一次同步用的签名(`根目录 + (名字, 是否目录, 层级)` 全量序列):变了才重建 items。
    /// ⚠️ `None` = 还没同步过(用空 `Vec` 表示会让首次同步被当成「没变」跳掉)。
    sig: Option<(Option<PathBuf>, Vec<(SharedString, bool, usize)>)>,
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
        let path_input = cx.new(|cx| InputState::new(window, cx).placeholder("输入目录路径"));
        // 用 `subscribe_in`:处理时要 `&mut Window`(写回输入框内容)。
        let input_sub = cx.subscribe_in(
            &path_input,
            window,
            |state: &mut Self, _, event, window, cx| state.on_path_input_event(event, window, cx),
        );
        Self {
            cwd: None,
            root: None,
            entries: None,
            expanded: Vec::new(),
            loading: Vec::new(),
            path_input,
            generation: 0,
            tree,
            sig: None,
            _tree_sub: sub,
            _input_sub: input_sub,
        }
    }

    /// 与当前会话的工作目录对齐(根视图每帧调用,`cwd` 是当前会话的进程工作目录)。
    ///
    /// 只在**目录真的变了**时动手:清空整棵树重新读一层,并让在途的读取作废。
    pub(crate) fn sync(
        &mut self,
        cwd: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.cwd == cwd {
            return;
        }
        self.cwd = cwd.clone();
        self.root = cwd;
        self.reset(window, cx);
    }

    /// 输入框改动 / 回车 ⇒ 尝试跳到里面的路径(回车让「同一个值也想再跳一次」也成立)。
    fn on_path_input_event(
        &mut self,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(event, InputEvent::Change | InputEvent::PressEnter { .. }) {
            return;
        }
        let text = self.path_input.read(cx).value().trim().to_string();
        self.navigate_to(&text, window, cx);
    }

    /// 跳到 `text` 指的目录:**必须确实存在且是目录**,否则什么都不动(空内容同理)。
    ///
    /// 「不存在就不动」是刻意的:打字中途必然经过一串不成立的中间态(把 `D:\a\b` 改成 `D:\c`
    /// 要先经过 `D:\a\c`),那时清空树只会让面板闪成空的。
    ///
    /// 相对路径按 `PathBuf` 常规解析(相对**进程**工作目录),不做「相对当前根目录」的改写。
    fn navigate_to(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        let target = PathBuf::from(text);
        if !target.is_dir() {
            return;
        }
        if self.root.as_deref() == Some(target.as_path()) {
            return;
        }
        self.root = Some(target);
        self.reset(window, cx);
    }

    /// 把当前根目录写回输入框(终端 `cd`、自己跳转成功后都走这里)。
    ///
    /// ⚠️ 用 `InputState::set_value`:它内部关掉事件发射 ⇒ 不会回环触发
    /// [`FilesState::on_path_input_event`]。内容已一致时直接返回,免得把光标拽到末尾。
    fn sync_path_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self
            .root
            .as_ref()
            .map(|root| root.display().to_string())
            .unwrap_or_default();
        // ⚠️ 先把 `read` 的借用收进一条语句(它返回 `Ref` 守卫,留在 `if` 里会一直持有),
        // 否则下面 `update` 会撞成重入借用。
        let current = self.path_input.read(cx).value();
        if current.as_ref() == text {
            return;
        }
        self.path_input
            .update(cx, |input, cx| input.set_value(text, window, cx));
    }

    /// 清空整棵树并重新读根目录(换目录时用)。
    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 代次 +1 ⇒ 在途的读取回来会被丢掉,不会把旧目录的内容填进新树。
        self.generation += 1;
        self.entries = None;
        self.expanded.clear();
        self.sig = None;
        // 根目录变了就可能把输入框一起对齐(内容一致时 `sync_path_input` 自己会跳过)。
        self.sync_path_input(window, cx);
        if let Some(dir) = self.root.clone() {
            self.spawn_load(dir, LoadTarget::Root, cx);
        }
        // 立刻把(暂时为空的)树推给 `TreeState`,免得旧目录的行残留到新目录读回来为止。
        self.sync_tree(cx);
        cx.notify();
    }

    /// 后台读一个目录(一层),读完回填(根目录 / 某个刚展开的目录);回填前核对代次。
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
                // 必须先 clone 再进 async 块(否则借用 `cx` 的生命周期过不了)。
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
            if let Some(dir) = pending {
                self.spawn_load(dir, LoadTarget::Dir(path), cx);
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
        // 没有目录可显示(终端全关掉 / 远端会话 / 本地目录还没采到 ⇒ `root` 为 `None`):
        // 只摆空占位,**连顶部那条路径输入框也不摆** —— 一条空框既没内容可编辑、也没东西可跳。
        if self.root.is_none() {
            return empty_state(
                IconName::FolderClosed,
                "没有可浏览的目录",
                Some("打开一个本地终端后,这里会显示它的工作目录;远端会话拿不到目录"),
            )
            .into_any_element();
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
                        // 不要清除按钮(路径跟着终端走,一键清空只会把面板弄空)。
                        .cleanable(false)
                        .aria_label("当前目录路径"),
                ),
            );

        // 目录里一条都没有:换成空占位(输入框留着 —— 还能靠它跳去别的目录)。
        let body: AnyElement = if rows == 0 {
            empty_state(IconName::FolderOpen, "这个目录是空的", None).into_any_element()
        } else {
            tree(&self.tree, |ix, entry, selected, _window, cx| {
                file_tree_row(ix, entry, selected, cx)
            })
            .h(TREE_ROW_HEIGHT * rows as f32)
            .into_any_element()
        };

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
