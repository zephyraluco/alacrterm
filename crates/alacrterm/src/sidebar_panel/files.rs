//! 「文件管理器」视图:只供远端(SSH)会话的目录树,入口 [`FilesState`]。
//! 树摊平成一行一项交给侧边栏虚拟列表,摊平与行骨架走 [`super::shared`]。

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

/// 路径输入框那一行的高度。
const PATH_ROW_HEIGHT: Pixels = px(36.);

/// 条目路径:根目录开始的下标链(`[]` = 根)。
type FilePath = Vec<usize>;

/// 拼接远端路径。
fn join_dir(dir: &str, name: &str) -> String {
    format!("{}/{}", dir.trim_end_matches('/'), name)
}


struct FileNode {
    name: SharedString,
    path: String,
    is_dir: bool,
    children: Option<Vec<FileNode>>,
}

/// 把 SFTP 列出的一层目录转成节点。
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

/// 后台读取结果的回填目标。
enum LoadTarget {
    Root,
    Navigate(String),
    Dir(FilePath),
}


#[derive(Clone)]
struct FileRow {
    path: FilePath,
    label: SharedString,
    depth: usize,
    kind: RowKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Dir { expanded: bool },
    File,
    Loading,
}

/// 「文件管理器」视图的状态。
pub(crate) struct FilesState {
    fs: Option<SshFs>,
    root: Option<String>,
    entries: Option<Vec<FileNode>>,
    error: Option<String>,
    expanded: Vec<FilePath>,
    loading: Vec<String>,
    selected: Option<FilePath>,
    path_input: Entity<InputState>,
    path_input_pending: bool,
    nav_seq: u64,
    generation: u64,
    rows: RowCache<FileRow>,
    _input_sub: Subscription,
}

impl FilesState {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let path_input = cx.new(|cx| InputState::new(window, cx).placeholder("输入远端目录路径"));
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

    /// 与当前会话对齐。
    pub(crate) fn sync(&mut self, fs: Option<SshFs>, window: &mut Window, cx: &mut Context<Self>) {
        if self.path_input_pending {
            self.path_input_pending = false;
            self.sync_path_input(window, cx);
        }
        if self.fs == fs {
            return;
        }
        self.fs = fs;
        self.root = None;
        self.reset(window, cx);
    }

    /// 输入框改动 / 回车时跳转。
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

    /// 跳到 `text` 指的目录,只认最新一次请求。
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

    fn sync_path_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.root.clone().unwrap_or_default();
        let current = self.path_input.read(cx).value();
        if current.as_ref() == text {
            return;
        }
        self.path_input
            .update(cx, |input, cx| input.set_value(text, window, cx));
    }

    /// 重置:清空整棵树并重读根目录。
    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_path_input(window, cx);
        self.reset_no_window(cx);
    }

    /// 重置状态并按需重读根目录。
    fn reset_no_window(&mut self, cx: &mut Context<Self>) {
        self.path_input_pending = true;
        self.generation += 1;
        self.entries = None;
        self.error = None;
        self.expanded.clear();
        self.loading.clear();
        self.selected = None;
        self.rows.bump();
        match (self.fs.clone(), self.root.clone()) {
            (Some(fs), Some(root)) => self.spawn_load(fs, root, LoadTarget::Root, cx),
            (Some(fs), None) => self.spawn_home(fs, cx),
            (None, _) => {}
        }
        cx.notify();
    }

    /// 取远端家目录作为起始根目录。
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

    /// 后台读一个目录,读完按 [`LoadTarget`] 回填。
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
                let mut cx = cx.clone();
                async move {
                    let result = task.await;
                    let _ = this.update(&mut cx, |this, cx| {
                        this.loading.retain(|pending| pending != &dir);
                        if this.generation != generation {
                            return;
                        }
                        this.rows.bump();
                        let entries = match result {
                            Ok(entries) => {
                                this.error = None;
                                nodes_from(&dir, entries)
                            }
                            Err(error) => {
                                this.error = Some(error.to_string());
                                Vec::new()
                            }
                        };
                        match &target {
                            LoadTarget::Root => this.entries = Some(entries),
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

    fn row(&self, ix: usize) -> Option<FileRow> {
        self.rows.row(ix)
    }

    /// 侧边栏虚拟列表的全部内容项。
    pub(super) fn sidebar_items(&mut self, cx: &mut Context<Self>) -> Vec<FilesItem> {
        if self.error.is_none() && (self.root.is_none() || self.entries.is_none()) {
            return Vec::new();
        }

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

    /// 点一行:目录切换展开,文件行记选中。
    fn activate_row(&mut self, path: FilePath, kind: RowKind, cx: &mut Context<Self>) {
        self.selected = Some(path.clone());
        if matches!(kind, RowKind::Dir { .. }) {
            let was_open = self.expanded.iter().any(|open| open == &path);
            self.expanded.retain(|open| open != &path);
            if !was_open {
                self.expanded.push(path.clone());
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

    fn node(&self, path: &[usize]) -> Option<&FileNode> {
        let mut nodes: &[FileNode] = self.entries.as_deref()?;
        let (last, parent) = path.split_last()?;
        for step in parent {
            nodes = nodes.get(*step)?.children.as_deref()?;
        }
        nodes.get(*last)
    }

    fn node_mut<'a>(nodes: &'a mut Vec<FileNode>, path: &[usize]) -> Option<&'a mut FileNode> {
        let (ix, rest) = path.split_first()?;
        if rest.is_empty() {
            return nodes.get_mut(*ix);
        }
        let node = nodes.get_mut(*ix)?;
        Self::node_mut(node.children.as_mut()?, rest)
    }
}


#[derive(Clone)]
pub(super) enum FilesItem {
    Path(Entity<InputState>),
    Row {
        state: WeakEntity<FilesState>,
        ix: usize,
    },
    Placeholder {
        icon: IconName,
        title: &'static str,
        description: &'static str,
        detail: Option<SharedString>,
    },
}

impl FilesItem {
    /// 画这一项。
    pub(super) fn render(self, cx: &mut App) -> AnyElement {
        match self {
            Self::Path(input) => h_flex()
                .w_full()
                .h(PATH_ROW_HEIGHT)
                .px_2()
                .child(
                    div().flex_1().min_w_0().child(
                        Input::new(&input)
                            .cleanable(false)
                            .aria_label("远端目录路径"),
                    ),
                )
                .into_any_element(),
            Self::Row { state, ix } => {
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


/// 画文件树的一行。
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
        RowKind::Loading => item.child(ellipsis_label(label)),
    }
}
