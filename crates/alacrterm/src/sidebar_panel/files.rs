//! 「文件管理器」视图:只供远端(SSH)会话的目录树,入口 [`FilesState`]。
//! 树摊平成一行一项交给侧边栏虚拟列表,摊平与行骨架走 [`super::shared`]。
//!
//! ## 八个动作
//!
//! 入口是面板**顶部那一行工具栏**(`sidebar_panel::activity_bar::files_toolbar`,照 MobaXterm
//! 的摆法:按钮在上、路径框在下),加上两套右键菜单。
//!
//! - **跳目录**(路径框 / 跟着终端走 / 工具栏「上一目录」,都走 [`FilesState::navigate_to`]):
//!   先问远端是不是目录,是才换根。
//! - **打开 / 编辑**(双击文件行 / 行右键「打开」/ 工具栏「编辑」):先把远端文件下到本地缓存
//!   ([`FilesState::open_file`])。下完之后按类型分两条路:文本 / 图片 / 其他交给
//!   [`crate::preview_window`] 那个独立窗口(先下再看是为了让预览只管一个本地路径);
//!   **音频 / 视频直接交给系统默认程序** —— 应用里没有解码器,见 [`FileKind`]。
//! - **下载**(工具栏「下载」/ 行右键「另存为…」= [`FilesState::download_as`],**路径问用户**;
//!   行右键「下载到下载目录」/ 工具栏「更多」里的同名项 = [`FilesState::download_to_download_dir`]):
//! - **上传**(拖进侧边栏 / 右键「上传文件…」/ 工具栏「上传」):落到当前选中项所在的远端目录。
//! - **新建文件夹 / 新建文件**(工具栏;名字走 [`crate::dialog::name`]):建在**当前根目录**下。
//! - **重命名**(工具栏;名字对话框预填旧名):只改名字,不挪位置。
//! - **删除**(工具栏;先弹确认):目录**整棵递归**(远端那一侧见 `ssh::fs` 的 `remove`)。
//!
//! 建 / 删 / 改名这三类都是**一步就完**的远端调用(没有进度),所以不进传输列表 —— 成功了
//! 重读一遍当前目录就算反馈。
//!
//! ## 传输进度
//!
//! 上传 / 下载都是 [`SshFs`] 的 `Transfer`(见 `crates/ssh/src/fs.rs`):进度与结果走同一条
//! `futures` 通道。这里把每条传输摊成一行([`FilesItem::Transfer`])摆在路径框下面,读通道的
//! 任务负责推进那一行、传完重读受影响的那个目录。
//!
//! ⚠️ **成功不推通知**:进度行消失 + 列表重读就是反馈(传上去的文件会自己出现在树里),
//! 弹 toast 只会盖住用户正在看的东西;失败才推(见 [`FilesState::report_error`])。

use std::path::PathBuf;
use std::rc::Rc;

use futures::StreamExt as _;
use gpui::{
    AnyElement, App, AppContext as _, AsyncApp, ClipboardItem, Context, CursorStyle, Entity,
    IntoElement, ParentElement as _, PathPromptOptions, Pixels, SharedString, Styled as _,
    Subscription, WeakEntity, Window, div, px,
};
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    list::ListItem,
    menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem},
    notification::Notification,
    progress::Progress,
    v_flex,
};
use terminal::{CancelToken, RemoteEntry, SshError, SshFs, Transfer, TransferEvent, TransferProgress};

use super::empty_state;
use super::shared::{RowCache, ellipsis_label, row_content, row_shell};
use crate::AppRoot;
use crate::assets::IconName;
use crate::format;
use crate::preview_window::{FileKind, PreviewRequest};

/// 路径输入框那一行的高度。
const PATH_ROW_HEIGHT: Pixels = px(36.);

/// 条目路径:根目录开始的下标链(`[]` = 根)。
type FilePath = Vec<usize>;

/// 拼接远端路径。
fn join_dir(dir: &str, name: &str) -> String {
    format!("{}/{}", dir.trim_end_matches('/'), name)
}

/// 上一级目录；`/`（或第一层）没有上一级就返回 `None`。
fn parent_dir(path: &str) -> Option<String> {
    let (parent, _) = path.trim_end_matches('/').rsplit_once('/')?;
    Some(if parent.is_empty() {
        "/".to_string()
    } else {
        parent.to_string()
    })
}

/// 把终端报来的目录展开成远端**绝对路径**；认不出（家目录还不知道）就给 `None`。
///
/// 终端有两条来源（见 `terminal::shell_integration`）：精确的 OSC 7 一定是绝对路径，
/// 而**窗口标题**那条会把家目录缩成 `~`（那是 bash `\w` 的行为，Debian / Ubuntu 默认
/// `PS1` 就是这个）。`~` 本身要家目录才展开得了，而家目录只有这里（SFTP）问得到。
fn expand_home(directory: &str, home: Option<&str>) -> Option<String> {
    match (directory, home) {
        ("~", Some(home)) => Some(home.to_string()),
        (directory, Some(home)) => directory
            .strip_prefix("~/")
            .map(|rest| join_dir(home, rest))
            .or_else(|| Some(directory.to_string())),
        // 家目录还不知道：绝对路径照用，`~` 那条等下一帧（家目录问回来得很快）。
        (directory, None) => directory.starts_with('/').then(|| directory.to_string()),
    }
}

/// 预览缓存的落地目录:`<系统缓存>/alacrterm/preview`。
///
/// 用缓存目录而不是 `temp_dir()`:有些平台会在开窗口的当口清理临时目录,而缓存目录至少
/// 是有归属的(预览窗口用完自己删,见 `preview_window::PreviewWindow::drop`;交给系统程序
/// 打开的那份留着,下次启动统一清,见 [`clear_preview_cache`])。
fn preview_dir() -> PathBuf {
    let base = dirs::cache_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(std::env::temp_dir);
    base.join("alacrterm").join("preview")
}

/// 清掉上次跑剩下的预览缓存(整个 `<缓存>/alacrterm/preview` 目录)。启动时调一次。
///
/// 缓存里那两种东西的生命周期都只到「这次打开」为止:预览窗口用的那份在窗口 `Drop` 时删;
/// **交给系统程序打开**的那份(音频 / 视频)得留给播放器读,应用这边不知道它什么时候读完 ——
/// 所以统一放到**下次启动**来收:那一刻还没有任何窗口在用它们,剩下的只可能是上次跑留下的
/// (包括上次崩了没清干净的情况)。⚠️ 还在被上一轮进程的播放器占着的文件删不掉,忽略即可
/// (下次启动再清)。⚠️ 这里顺手把**没开成的预览窗口**留下的那份也清了,所以调用点要在
/// 任何窗口打开**之前**。
pub(crate) fn clear_preview_cache() {
    let dir = preview_dir();
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => log::debug!("已清掉预览缓存:{}", dir.display()),
        // 还没打开过任何文件时目录本来就不存在,这是最常见的情况。
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => log::warn!("清理预览缓存失败:{}:{error}", dir.display()),
    }
}

/// 系统的下载目录(问不到就退回当前目录)。
fn download_dir() -> PathBuf {
    dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 把一个远端文件名变成安全的本地文件名(远端路径里不该有分隔符,防一手)。
fn safe_file_name(name: &str) -> String {
    name.replace(['/', '\\'], "_")
}

/// 目标路径已经被占了就在主名后面加 ` (1)` / ` (2)`…(不回问用户,也不覆盖)。
fn unique_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let parent = path.parent().map(PathBuf::from).unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let extension = path.extension().map(|ext| ext.to_string_lossy().into_owned());
    for n in 1..1000 {
        let name = match &extension {
            Some(extension) => format!("{stem} ({n}).{extension}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    path
}

/// 在树里找某个远端目录的下标链(只找已经读过的那部分)。
fn find_dir_in(nodes: &[FileNode], remote: &str) -> Option<FilePath> {
    for (ix, node) in nodes.iter().enumerate() {
        if !node.is_dir {
            continue;
        }
        if node.path == remote {
            return Some(vec![ix]);
        }
        if let Some(mut nested) = node
            .children
            .as_deref()
            .and_then(|children| find_dir_in(children, remote))
        {
            nested.insert(0, ix);
            return Some(nested);
        }
    }
    None
}


struct FileNode {
    name: SharedString,
    path: String,
    is_dir: bool,
    size: u64,
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
            size: entry.size,
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
    /// 远端路径(右键菜单里的「复制远端路径」用)。
    remote: String,
    depth: usize,
    kind: RowKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Dir { expanded: bool },
    File,
    Loading,
}

/// 一条在跑的传输,对应侧边栏里的一行。
#[derive(Clone)]
struct TransferState {
    id: u64,
    /// 条目名(或「n 个文件」)。
    label: SharedString,
    files_done: u64,
    files_total: u64,
    bytes_done: u64,
    bytes_total: u64,
    cancel: CancelToken,
}

impl TransferState {
    /// 进度百分比(0 ~ 100)。
    ///
    /// 优先按字节算(比按文件个数准得多);总字节数不知道(空文件 / 服务端没报大小)时退化成
    /// 「第几个文件」。
    fn percent(&self) -> f32 {
        if self.bytes_total > 0 {
            (self.bytes_done as f32 / self.bytes_total as f32 * 100.).clamp(0., 100.)
        } else if self.files_total > 0 {
            (self.files_done as f32 / self.files_total as f32 * 100.).clamp(0., 100.)
        } else {
            0.
        }
    }

    /// 行右侧那行说明文字。
    fn detail(&self) -> String {
        if self.bytes_total > 0 {
            format!(
                "{} / {}",
                format::size(self.bytes_done),
                format::size(self.bytes_total)
            )
        } else if self.files_total > 1 {
            format!("{}/{} 个", self.files_done, self.files_total)
        } else {
            format::size(self.bytes_done)
        }
    }
}

/// 传输跑完之后要做什么。
///
/// ⚠️ 这里**不再带通知文本**了:成功一律静默(进度行消失即反馈),失败由
/// [`FilesState::report_error`] 统一报。
enum TransferDone {
    /// 打开预览窗口(文本 / 图片 / 其他下完缓存后走这条)。
    Preview(PreviewRequest),
    /// 交给系统默认程序打开本地那份(音频 / 视频走这条)。
    OpenExternally(PathBuf),
    /// 只推一条通知。
    Nothing,
}

/// 「文件管理器」顶部工具栏那几枚按钮要的判据（见 `sidebar_panel::activity_bar::files_toolbar`）。
///
/// 每帧现取一次（[`FilesState::status`]），所以「跳目录了 / 选中项变了 / 会话断了」按钮
/// 就跟着变灰。
#[derive(Clone)]
pub(super) struct FilesStatus {
    /// 会话可用（远端已连上、而且根目录已知）：为假时整套按钮全灰。
    pub(super) ready: bool,
    /// 根目录还有上一级（`/` 就没有）。
    pub(super) can_go_up: bool,
    /// 选中的那个条目叫什么（重命名预填 / 删除确认用）。`None` = 没选中。
    pub(super) selected: Option<SharedString>,
    /// 选中的是个目录（「下载」与「删除」提示语要它）。
    pub(super) selected_is_dir: bool,
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
    /// 上一次「跟着终端走」跟着的那个目录（见 [`FilesState::follow_terminal`]）。
    ///
    /// 即「终端上一次报的目录」:shell 每次画提示符都会报一遍同一个目录,靠它去重;
    /// 用户在路径框里跳到别处、而 shell 没动时,也不会被拽回去。
    followed: Option<String>,
    /// 远端家目录:终端报的目录可能是 `~` / `~/x`(见
    /// [`FilesState::follow_terminal`]),展开要用它。问到之前为 `None`。
    home: Option<String>,
    nav_seq: u64,
    generation: u64,
    rows: RowCache<FileRow>,
    /// 在跑的传输(最新的在前,见 [`FilesState::spawn_transfer`])。
    transfers: Vec<TransferState>,
    next_transfer_id: u64,
    /// 预览缓存的递增序号(同名文件互不覆盖,见 [`FilesState::open_file`])。
    cache_seq: u64,
    /// 拿它开预览窗口 / 推通知(见 [`FilesState::open_file`])。
    app_root: WeakEntity<AppRoot>,
    _input_sub: Subscription,
}

impl FilesState {
    pub(crate) fn new(
        window: &mut Window,
        app_root: WeakEntity<AppRoot>,
        cx: &mut Context<Self>,
    ) -> Self {
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
            followed: None,
            home: None,
            nav_seq: 0,
            generation: 0,
            rows: RowCache::default(),
            transfers: Vec::new(),
            next_transfer_id: 0,
            cache_seq: 0,
            app_root,
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
        // 换了会话就忘了「上次跟到哪儿」与旧会话的家目录:新会话的目录跟旧会话没关系。
        self.followed = None;
        self.home = None;
        // ⚠️ 传输行清掉,但在途的那些传输**不会**停(它们活在 worker 的那条连接上)。这是
        // 有意取舍:会话已经换掉了,行摆着也没处显示;worker 那边下次请求自然会用到新连接。
        self.transfers.clear();
        self.reset(window, cx);
    }

    /// 跟着终端走：远端 shell 报了个新目录就跳过去。
    ///
    /// 数据源是终端自己说出来的当前目录（见 `terminal::shell_integration`：精确的 OSC 7，
    /// 或 Ubuntu 默认 `PS1` 放在**窗口标题**里的 `\w`）。每帧都会调一次（见
    /// [`crate::AppRoot::render`]），所以这里得是幂等的：
    ///
    /// - `self.root == directory` 时什么也不做（绝大多数帧都属于这种情况）；
    /// - 报的是**同一个**目录、但用户已经手动跳到别处时也不动（`followed` 去重）——
    ///   跟着走的意思是「shell `cd` 了我也 `cd`」，不是「把用户拽回 shell 那儿」；
    /// - 真的换了目录才跳，并记住它。
    ///
    /// ⚠️ 标题那条来源会把家目录缩成 `~`（`\w` 的行为），所以这里先展开（见
    /// [`expand_home`]）；家目录还没问到就先不动，等下一帧。
    pub(crate) fn follow_terminal(&mut self, directory: Option<&str>, cx: &mut Context<Self>) {
        let Some(directory) = directory else {
            return;
        };
        let Some(directory) = expand_home(directory, self.home.as_deref()) else {
            return;
        };
        let directory = directory.as_str();
        if self.fs.is_none()
            || self.root.as_deref() == Some(directory)
            || self.followed.as_deref() == Some(directory)
        {
            return;
        }
        self.followed = Some(directory.to_string());
        self.navigate_to(directory, cx);
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
                            this.home = Some(home.clone());
                            // 终端已经报过目录、并且已经跳过去了(见 `follow_terminal`):
                            // 别拿家目录盖回来 —— 两条路都是异步的,谁先回来不一定。
                            if this.root.is_some() {
                                return;
                            }
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
                remote: node.path.clone(),
                depth: path.len() - 1,
                kind,
            });
            if let RowKind::Dir { expanded: true } = kind {
                match &node.children {
                    Some(children) => Self::flatten(children, expanded, path, out),
                    None => out.push(FileRow {
                        path: path.clone(),
                        label: "加载中…".into(),
                        remote: node.path.clone(),
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

    /// 按 id 取一条在跑的传输(渲染传输行时用)。
    fn transfer(&self, id: u64) -> Option<TransferState> {
        self.transfers.iter().find(|t| t.id == id).cloned()
    }

    // --------------------------------------------------------- 打开 / 上传 / 下载

    /// 「打开」:先把远端文件下到本地缓存,下完按类型交给预览窗口或系统程序。
    ///
    /// 只处理文件 —— 行上的双击与「打开」菜单项都只对文件摆。文本在那个窗口里**能改**,
    /// Ctrl+S 把内容写回远端(见 [`crate::preview_window::PreviewWindow::save`]),所以请求里
    /// 还要捎上 [`SshFs`] 句柄与根实体(后者用来在写回之后让本视图把所在目录重读一次)。
    ///
    /// ⚠️ **音频 / 视频不开窗口**:应用里没有解码器(内嵌播放要么自己写一层 unsafe 的 COM,
    /// 要么引来 ffmpeg),下完直接交给系统默认程序。那份副本留在缓存目录里给播放器读,
    /// 由下次启动清掉(见 [`clear_preview_cache`])。
    pub(super) fn open_file(&mut self, path: FilePath, cx: &mut Context<Self>) {
        let (Some(fs), Some(node)) = (self.fs.clone(), self.node(&path)) else {
            return;
        };
        let (remote, name, size) = (node.path.clone(), node.name.clone(), node.size);
        self.cache_seq += 1;
        let local = preview_dir().join(format!("{}-{}", self.cache_seq, safe_file_name(&name)));
        let transfer = fs.download_one(remote.clone(), local.clone());
        let done = if FileKind::of(&name).handed_to_system() {
            TransferDone::OpenExternally(local)
        } else {
            TransferDone::Preview(PreviewRequest {
                name: name.clone(),
                host: fs.host().to_string(),
                remote_path: remote,
                local_path: local,
                size,
                fs,
                app_root: self.app_root.clone(),
            })
        };
        self.spawn_transfer(name, None, transfer, done, cx);
    }

    /// 「下载到…」:文件走系统的**另存为**,目录走选目录 + 整个下下来。
    ///
    /// ⚠️ 目标位置要问系统(异步),所以先拿通道、再在任务里等用户选完才真的开传。
    pub(super) fn download_as(&mut self, path: FilePath, cx: &mut Context<Self>) {
        let (Some(fs), Some(node)) = (self.fs.clone(), self.node(&path)) else {
            return;
        };
        let (remote, name, is_dir) = (node.path.clone(), node.name.clone(), node.is_dir);
        let state = cx.weak_entity();

        if is_dir {
            let picked = cx.prompt_for_paths(PathPromptOptions {
                files: false,
                directories: true,
                multiple: false,
                prompt: Some("选择下载到哪个目录".into()),
            });
            cx.spawn(move |_: WeakEntity<Self>, cx: &mut AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    let Ok(Ok(Some(mut picked))) = picked.await else {
                        return;
                    };
                    let Some(dir) = picked.pop() else {
                        return;
                    };
                    let _ = state.update(&mut cx, |this, cx| {
                        // 远端目录名会落在选中的目录下面。
                        let transfer = fs.download(vec![remote.clone()], dir);
                        this.spawn_transfer(name.clone(), None, transfer, TransferDone::Nothing, cx);
                    });
                }
            })
            .detach();
            return;
        }

        let picked = cx.prompt_for_new_path(&download_dir(), Some(name.as_ref()));
        cx.spawn(move |_: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                let Ok(Ok(Some(target))) = picked.await else {
                    return;
                };
                let _ = state.update(&mut cx, |this, cx| {
                    let transfer = fs.download_one(remote.clone(), target);
                    this.spawn_transfer(name.clone(), None, transfer, TransferDone::Nothing, cx);
                });
            }
        })
        .detach();
    }

    /// 「下载到下载目录」:不问路径,直接落到系统下载目录(单个文件重名了自动改名)。
    pub(super) fn download_to_download_dir(&mut self, path: FilePath, cx: &mut Context<Self>) {
        let (Some(fs), Some(node)) = (self.fs.clone(), self.node(&path)) else {
            return;
        };
        let dir = download_dir();
        let transfer = if node.is_dir {
            fs.download(vec![node.path.clone()], dir)
        } else {
            // 先算一个不撞的名字,免得悄悄覆盖用户已有的文件。
            fs.download_one(node.path.clone(), unique_path(dir.join(node.name.as_ref())))
        };
        self.spawn_transfer(node.name.clone(), None, transfer, TransferDone::Nothing, cx);
    }

    /// 「上传…」:把本地这些路径传进当前选中项所在的远端目录。
    ///
    /// 目标目录 = 选中目录 → 它本身;选中文件 → 它的父目录;什么都没选 → 当前根目录
    /// (见 [`Self::upload_target_dir`])。拖放上传走的也是这里。
    pub(super) fn start_upload(&mut self, locals: Vec<PathBuf>, cx: &mut Context<Self>) {
        if locals.is_empty() {
            return;
        }
        let (Some(fs), Some(dir)) = (self.fs.clone(), self.upload_target_dir()) else {
            return;
        };
        let label: SharedString = match locals.as_slice() {
            [one] => one
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| one.to_string_lossy().into_owned())
                .into(),
            many => format!("{} 个文件", many.len()).into(),
        };
        let transfer = fs.upload(locals, dir.clone());
        self.spawn_transfer(label, Some(dir), transfer, TransferDone::Nothing, cx);
    }

    /// 上传的落地目录。
    fn upload_target_dir(&self) -> Option<String> {
        let Some(node) = self.selected.as_deref().and_then(|path| self.node(path)) else {
            return self.root.clone();
        };
        if node.is_dir {
            return Some(node.path.clone());
        }
        // 选中文件 ⇒ 传进它所在的那个目录。`join_dir` 会把空目录名拼成 `/名字`,所以
        // 「根目录下的文件」这种边界不用特判。
        Some(
            node.path
                .rsplit_once('/')
                .map(|(dir, _)| dir.to_string())
                .unwrap_or_else(|| node.path.clone()),
        )
    }

    /// 「刷新」:重读当前根目录(远端可能被别处改过)。
    pub(super) fn refresh(&mut self, cx: &mut Context<Self>) {
        if let (Some(fs), Some(root)) = (self.fs.clone(), self.root.clone()) {
            self.spawn_load(fs, root, LoadTarget::Root, cx);
        }
    }

    /// 跳到上一级目录（状态栏那枚按钮；根目录是 `/` 时什么也不做）。
    ///
    /// 走的是与路径框、`follow_terminal` 同一条路（[`Self::navigate_to`]：先问远端是不是目录，
    /// 是才跳），所以这里的 `parent_dir` 只是个字符串切分，切出来的东西由远端把关。
    pub(super) fn navigate_up(&mut self, cx: &mut Context<Self>) {
        let Some(parent) = self.root.as_deref().and_then(parent_dir) else {
            return;
        };
        self.navigate_to(&parent, cx);
    }

    /// 「下载」那枚按钮：把**当前选中项**下下来，**路径问用户**（没选中就什么也不做）。
    ///
    /// 走的就是行右键菜单那两项的同一条路（[`Self::download_as`]）：文件弹系统「另存为」、
    /// 目录弹「选择下载到哪个目录」再把整棵下下来。⚠️ 不要改成 `download_to_download_dir`
    /// （不问路径直落系统下载目录）—— 那是行右键菜单里另一项的事。
    pub(super) fn download_selected(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.selected.clone() else {
            return;
        };
        self.download_as(path, cx);
    }

    /// 「编辑」那枚按钮 / 「打开」菜单项：选中的是文件就 [`Self::open_file`]，是目录就
    /// 当作点它一下（切换展开）。
    pub(super) fn open_selected(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.selected.clone() else {
            return;
        };
        let Some(is_dir) = self.node(&path).map(|node| node.is_dir) else {
            return;
        };
        if is_dir {
            // `activate_row` 对目录是「切换展开」，`expanded` 那个字段它不看。
            self.activate_row(path, RowKind::Dir { expanded: false }, cx);
        } else {
            self.open_file(path, cx);
        }
    }

    // --------------------------------------------------------- 新建 / 重命名 / 删除

    /// 在当前根目录下新建一个文件夹（名字来自「新建文件夹」对话框）。
    pub(super) fn create_dir(&mut self, name: String, cx: &mut Context<Self>) {
        let (Some(fs), Some(root)) = (self.fs.clone(), self.root.clone()) else {
            return;
        };
        // ⚠️ 名字里带 `/` 会让它落到别的目录去，直接挡掉（与新建文件夹的语义不符）。
        if name.contains('/') {
            self.report_error(cx, format!("名字里不能有 / ：{name}"));
            return;
        }
        let path = join_dir(&root, &name);
        self.spawn_remote_edit(name.into(), async move { fs.mkdir(path).await }, cx);
    }

    /// 在当前根目录下新建一个空文件（名字来自「新建文件」对话框）。
    pub(super) fn create_file(&mut self, name: String, cx: &mut Context<Self>) {
        let (Some(fs), Some(root)) = (self.fs.clone(), self.root.clone()) else {
            return;
        };
        if name.contains('/') {
            self.report_error(cx, format!("名字里不能有 / ：{name}"));
            return;
        }
        let path = join_dir(&root, &name);
        self.spawn_remote_edit(name.into(), async move { fs.create_file(path).await }, cx);
    }

    /// 把选中的这一项改名（新名字来自「重命名」对话框）。
    ///
    /// 移到自己原来的名字上就当没改（对话框里直接按了确认）。⚠️ 改名后**清掉选中**：
    /// `selected` 存的是下标链，重读之后那个下标已经是别的条目了。
    pub(super) fn rename_selected(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(path) = self.selected.clone() else {
            return;
        };
        let Some(node) = self.node(&path) else {
            return;
        };
        let (Some(fs), Some(dir)) = (self.fs.clone(), parent_dir(&node.path)) else {
            return;
        };
        if name.contains('/') {
            self.report_error(cx, format!("名字里不能有 / ：{name}"));
            return;
        }
        let from = node.path.clone();
        let to = join_dir(&dir, &name);
        if from == to {
            return;
        }
        self.selected = None;
        self.rows.bump();
        self.spawn_remote_edit(name.into(), async move { fs.rename(from, to).await }, cx);
    }

    /// 删掉选中的这一项（目录**整棵递归**；确认对话框在工具栏那边弹）。
    ///
    /// ⚠️ 走的是**传输通道**（与上传 / 下载同一条，不是 [`Self::spawn_remote_edit`]）:
    /// 一棵装满东西的子树删起来可能很久，这样才有**进度行**（已删几个 / 共几个，带取消）,
    /// 而且不会把浏览那条连接占住。
    pub(super) fn remove_selected(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.selected.clone() else {
            return;
        };
        let Some(node) = self.node(&path) else {
            return;
        };
        let (Some(fs), Some(root)) = (self.fs.clone(), self.root.clone()) else {
            return;
        };
        let (label, remote, is_dir) = (node.name.clone(), node.path.clone(), node.is_dir);
        self.selected = None;
        self.rows.bump();
        let transfer = fs.remove(remote, is_dir);
        // 删完重读当前根目录（那一行得从树上消失）。
        self.spawn_transfer(label, Some(root), transfer, TransferDone::Nothing, cx);
    }

    /// 跑一个「改远端」的短操作：成功了重读当前目录，失败了推一条错误通知。
    ///
    /// **新建文件夹 / 新建文件 / 重命名**三条走这里 —— 它们都是**单次往返**的远端调用,
    /// 快到不存在「占住浏览连接」的问题,所以不进传输列表(也不该排在正在跑的大文件后面)。
    /// ⚠️ **删除不走这里**(它可能很久,走传输通道;见 [`Self::remove_selected`])。
    fn spawn_remote_edit(
        &mut self,
        label: SharedString,
        task: impl std::future::Future<Output = Result<(), SshError>> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let task = cx.background_spawn(task);
        cx.spawn(move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                let result = task.await;
                let _ = this.update(&mut cx, |this, cx| match result {
                    // 成功了就重读当前目录:新条目 / 少掉的条目 / 改过的名字这一下就对上了。
                    Ok(()) => this.refresh(cx),
                    Err(error) => {
                        this.report_error(cx, format!("{label} 操作失败:{error}"));
                    }
                });
            }
        })
        .detach();
    }

    /// 推一条错误通知（让 [`crate::AppRoot`] 转给窗口）与一条日志。
    ///
    /// ⚠️ 成功那一路**不推**通知（见 [`Self::on_transfer_done`] 与这里同一个取舍）:
    /// 列表自己重读一遍就是反馈,弹 toast 只会盖住用户正在看的东西。失败必须说 ——
    /// 不说的话用户不知道要重试。
    fn report_error(&self, cx: &mut App, message: String) {
        log::warn!("{message}");
        let _ = self
            .app_root
            .update(cx, |root, cx| root.notify(Notification::error(message), cx));
    }

    // --------------------------------------------------------- 传输

    /// 起一条传输:立刻摆一行进度,再后台读通道推进那一行。
    fn spawn_transfer(
        &mut self,
        label: SharedString,
        refresh: Option<String>,
        transfer: Transfer,
        done: TransferDone,
        cx: &mut Context<Self>,
    ) {
        let id = self.next_transfer_id;
        self.next_transfer_id += 1;
        let Transfer { mut events, cancel } = transfer;
        // 最新的摆最上面。
        self.transfers.insert(
            0,
            TransferState {
                id,
                label,
                files_done: 0,
                files_total: 0,
                bytes_done: 0,
                bytes_total: 0,
                cancel,
            },
        );
        cx.notify();

        let state = cx.weak_entity();
        cx.spawn(move |_: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                let mut done = Some(done);
                let mut finished = false;
                while let Some(event) = events.next().await {
                    match event {
                        TransferEvent::Progress(progress) => {
                            let keep = state
                                .update(&mut cx, |this, cx| {
                                    this.on_transfer_progress(id, progress, cx)
                                })
                                .unwrap_or(false);
                            // 行没了 = 用户取消了,不用再读。
                            if !keep {
                                break;
                            }
                        }
                        TransferEvent::Done(result) => {
                            finished = true;
                            let Some(done) = done.take() else {
                                break;
                            };
                            let _ = state.update(&mut cx, |this, cx| {
                                this.on_transfer_done(id, result, refresh.clone(), done, cx)
                            });
                            break;
                        }
                    }
                }
                // 通道直接断了(worker 线程没了):把那一行收掉并说一声,别让它一直挂着。
                if !finished {
                    let _ = state.update(&mut cx, |this, cx| this.on_transfer_lost(id, cx));
                }
            }
        })
        .detach();
    }

    fn on_transfer_progress(
        &mut self,
        id: u64,
        progress: TransferProgress,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(transfer) = self.transfers.iter_mut().find(|t| t.id == id) else {
            return false;
        };
        transfer.label = progress.label.into();
        transfer.files_done = progress.files_done;
        transfer.files_total = progress.files_total;
        transfer.bytes_done = progress.bytes_done;
        transfer.bytes_total = progress.bytes_total;
        cx.notify();
        true
    }

    /// 传完了(或被取消 / 失败):收掉那一行,再把受影响的目录重读一次。
    ///
    /// ⚠️ **成功不推通知**:进度行消失 + 列表重读一遍就是反馈(传上去的文件会自己出现在树里),
    /// 弹 toast 只会盖住用户正在看的东西。失败必须说 —— 不说用户不知道要重试。
    fn on_transfer_done(
        &mut self,
        id: u64,
        result: Result<(), SshError>,
        refresh: Option<String>,
        done: TransferDone,
        cx: &mut Context<Self>,
    ) {
        // 行已经没了 ⇒ 这次是被取消的,Done 只是 worker 收尾,不用再打扰用户。
        let Some(transfer) = self.remove_transfer(id) else {
            return;
        };
        let label = transfer.label;
        match result {
            Ok(()) => match done {
                // 预览窗口本身就是反馈。
                TransferDone::Preview(request) => {
                    let _ = self
                        .app_root
                        .update(cx, |root, cx| root.open_preview_window(request, cx));
                }
                // 系统程序在应用外面开。
                TransferDone::OpenExternally(path) => cx.open_with_system(&path),
                // 下载 / 上传完成:什么都不用做。
                TransferDone::Nothing => {}
            },
            Err(SshError::Cancelled) => {}
            Err(error) => self.report_error(cx, format!("{label} 传输失败:{error}")),
        }
        if let Some(dir) = refresh {
            self.reload_dir(&dir, cx);
        }
    }

    /// 通道在 `Done` 之前就断了:worker 线程没了。
    fn on_transfer_lost(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(transfer) = self.remove_transfer(id) else {
            return;
        };
        let message = format!("{} 传输中断:远端文件系统线程已退出", transfer.label);
        self.report_error(cx, message);
    }

    /// 取消一条传输(点进度行右边的「×」)。
    fn cancel_transfer(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(transfer) = self.transfers.iter().find(|t| t.id == id) {
            transfer.cancel.cancel();
        }
        self.remove_transfer(id);
        cx.notify();
    }

    /// 收掉一行;返回收掉的那条(`None` = 这个 id 已经不在列表里了)。
    fn remove_transfer(&mut self, id: u64) -> Option<TransferState> {
        let ix = self.transfers.iter().position(|t| t.id == id)?;
        Some(self.transfers.remove(ix))
    }

    /// 把某个远端目录重读一次(上传 / 下载完成后刷新它;预览窗口写回远端后也走这里)。
    pub(crate) fn reload_dir(&mut self, dir: &str, cx: &mut Context<Self>) {
        let Some(fs) = self.fs.clone() else {
            return;
        };
        if self.root.as_deref() == Some(dir) {
            self.spawn_load(fs, dir.to_string(), LoadTarget::Root, cx);
            return;
        }
        // 树里能找到(而且已经读过)的那个目录才刷得动;没展开过的本来就会在读的时候拿新的。
        let Some(path) = self
            .entries
            .as_deref()
            .and_then(|nodes| find_dir_in(nodes, dir))
        else {
            return;
        };
        self.spawn_load(fs, dir.to_string(), LoadTarget::Dir(path), cx);
    }

    /// 状态栏 / 工具栏那几枚按钮要的判据（见 [`FilesStatus`]）。
    pub(super) fn status(&self) -> FilesStatus {
        let selected = self.selected.as_deref().and_then(|path| self.node(path));
        FilesStatus {
            // 根目录还没问到时就当不可用：上传没处落、刷新也没得刷。
            ready: self.fs.is_some() && self.root.is_some(),
            can_go_up: self.root.as_deref().and_then(parent_dir).is_some(),
            selected: selected.map(|node| node.name.clone()),
            selected_is_dir: selected.is_some_and(|node| node.is_dir),
        }
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
        let mut items = Vec::with_capacity(rows.len() + self.transfers.len() + 2);
        items.push(FilesItem::Path(self.path_input.clone()));
        // 传输行摆在路径框下面、树上面:有东西在传时它最需要被看到。
        let state = cx.entity().downgrade();
        items.extend(self.transfers.iter().map(|transfer| FilesItem::Transfer {
            state: state.clone(),
            id: transfer.id,
        }));
        if rows.is_empty() {
            items.push(FilesItem::Placeholder {
                icon: IconName::FolderOpen,
                title: "这个目录是空的",
                description: "可以用上面的路径框跳到别的目录",
                detail: self.error.clone().map(SharedString::from),
            });
        } else {
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
    /// 一条在跑的传输(上传 / 下载)。
    Transfer {
        state: WeakEntity<FilesState>,
        id: u64,
    },
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
            Self::Transfer { state, id } => {
                let Ok(Some(transfer)) = state.read_with(&*cx, |files, _| files.transfer(id)) else {
                    return div().w_full().into_any_element();
                };
                transfer_element(id, &transfer, &state, cx)
            }
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
                let remote = row.remote.clone();
                let name = row.label.clone();
                let kind = row.kind;
                let is_dir = matches!(kind, RowKind::Dir { .. });
                let element = row_element(ix, &row, selected, cx);
                if matches!(kind, RowKind::Loading) {
                    return div().w_full().child(element).into_any_element();
                }
                let click_state = state.clone();
                let menu_state = state.clone();
                let open_path = path.clone();
                let menu_path = path.clone();
                div()
                    .w_full()
                    .child(
                        element
                            .on_click(move |event, _window, cx| {
                                if event.click_count() >= 2 && !is_dir {
                                    let _ = click_state
                                        .update(cx, |files, cx| files.open_file(open_path.clone(), cx));
                                    return;
                                }
                                let _ = click_state.update(cx, |files, cx| {
                                    files.activate_row(open_path.clone(), kind, cx)
                                });
                            })
                            .context_menu(move |menu, _window, _cx| {
                                let menu = row_menu(
                                    menu,
                                    &menu_state,
                                    menu_path.clone(),
                                    remote.clone(),
                                    name.clone(),
                                    is_dir,
                                );
                                menu
                            }),
                    )
                    .into_any_element()
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

/// 一个传输行:名字 + 进度 + 取消。
///
/// 这一行比树行高(28px):进度条要占掉一行,挤在 28px 里看不清。
fn transfer_element(
    id: u64,
    transfer: &TransferState,
    state: &WeakEntity<FilesState>,
    cx: &mut App,
) -> AnyElement {
    let percent = transfer.percent();
    let detail = transfer.detail();
    let radius = cx.theme().radius;
    v_flex()
        .w_full()
        .h(px(46.))
        .justify_center()
        .gap_1()
        .px_2()
        .rounded(radius)
        .child(
            h_flex()
                .gap_x_2()
                .items_center()
                .child(Icon::new(IconName::LoaderCircle).size_3())
                .child(div().flex_1().min_w_0().child(ellipsis_label(transfer.label.clone())))
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(detail),
                )
                .child({
                    let state = state.clone();
                    Button::new(format!("transfer-cancel-{id}"))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Close)
                        .tooltip("取消")
                        .on_click(move |_, _, cx| {
                            let _ = state.update(cx, |files, cx| files.cancel_transfer(id, cx));
                        })
                }),
        )
        .child(Progress::new(format!("transfer-bar-{id}")).value(percent))
        .into_any_element()
}

/// 一行的右键菜单。
///
/// 目录与文件的条目不同:目录没得「打开」(只展开),文件没得「下载文件夹」。
fn row_menu(
    menu: PopupMenu,
    state: &WeakEntity<FilesState>,
    path: FilePath,
    remote: String,
    name: SharedString,
    is_dir: bool,
) -> PopupMenu {
    let mut menu = menu;

    if !is_dir {
        let open_state = state.clone();
        let open_path = path.clone();
        menu = menu.item(
            PopupMenuItem::new("打开")
                .icon(IconName::FileText)
                .on_click(move |_, _, cx| {
                    let _ = open_state.update(cx, |files, cx| files.open_file(open_path.clone(), cx));
                }),
        );
    }

    menu = menu.item({
        let state = state.clone();
        let path = path.clone();
        let label = if is_dir { "下载文件夹到…" } else { "另存为…" };
        PopupMenuItem::new(label)
            .icon(IconName::Download)
            .on_click(move |_, _, cx| {
                let _ = state.update(cx, |files, cx| files.download_as(path.clone(), cx));
            })
    });

    menu = menu.item({
        let state = state.clone();
        let path = path.clone();
        let label: SharedString = if is_dir {
            "下载到下载目录".into()
        } else {
            format!("下载到下载目录/{name}").into()
        };
        PopupMenuItem::new(label)
            .icon(IconName::Download)
            .on_click(move |_, _, cx| {
                let _ = state.update(cx, |files, cx| {
                    files.download_to_download_dir(path.clone(), cx)
                });
            })
    });

    menu.separator().item({
        let copy_remote = remote.clone();
        PopupMenuItem::new("复制远端路径")
            .icon(IconName::Copy)
            .on_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(copy_remote.clone()));
            })
    })
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

#[cfg(test)]
mod tests {
    use super::{FileNode, find_dir_in, join_dir, parent_dir};
    use gpui::SharedString;

    fn dir(name: &str, path: &str, children: Option<Vec<FileNode>>) -> FileNode {
        FileNode {
            name: SharedString::from(name),
            path: path.to_string(),
            is_dir: true,
            size: 0,
            children,
        }
    }

    fn file(name: &str, path: &str) -> FileNode {
        FileNode {
            name: SharedString::from(name),
            path: path.to_string(),
            is_dir: false,
            size: 3,
            children: None,
        }
    }

    /// 上传 / 下载完要重读「受影响的那个目录」,靠这个函数在树里把它找回来。
    #[test]
    fn finds_a_directory_at_any_depth() {
        let tree = vec![
            dir(
                "home",
                "/home",
                Some(vec![
                    dir("me", "/home/me", Some(vec![dir("work", "/home/me/work", None)])),
                    file("notes.txt", "/home/me/notes.txt"),
                ]),
            ),
            file("readme", "/readme"),
        ];

        assert_eq!(find_dir_in(&tree, "/home"), Some(vec![0]));
        assert_eq!(find_dir_in(&tree, "/home/me"), Some(vec![0, 0]));
        assert_eq!(find_dir_in(&tree, "/home/me/work"), Some(vec![0, 0, 0]));
    }

    /// 没读过的目录(子节点还没拉)找不到 —— 它展开时会自己读新的,不需要刷。
    #[test]
    fn does_not_invent_directories_it_has_not_read() {
        let tree = vec![dir("home", "/home", None)];
        assert_eq!(find_dir_in(&tree, "/home/me"), None);
        // 文件不算命中(即使路径一样)。
        let with_file = vec![file("home", "/home")];
        assert_eq!(find_dir_in(&with_file, "/home"), None);
    }

    /// 状态栏那枚「上一目录」:根目录 `/` 没有上一级(所以按不了),其余往上一层层切。
    #[test]
    fn parent_dir_stops_at_the_root() {
        assert_eq!(parent_dir("/"), None);
        assert_eq!(parent_dir(""), None);
        assert_eq!(parent_dir("//"), None);
        assert_eq!(parent_dir("/home"), Some("/".to_string()));
        assert_eq!(parent_dir("/home/me"), Some("/home".to_string()));
        // 尾部的分隔符不算一层(路径框里手打 `/home/me/` 也该退到 `/home`)。
        assert_eq!(parent_dir("/home/me/"), Some("/home".to_string()));
    }

    /// 上一目录之后 `join_dir` 要能把名字拼回同一个路径(两个函数是配对的)。
    #[test]
    fn parent_dir_round_trips_with_join_dir() {
        let child = join_dir("/home/me", "work");
        assert_eq!(child, "/home/me/work");
        assert_eq!(parent_dir(&child), Some("/home/me".to_string()));
    }
}
