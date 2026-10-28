//! 「打开文件」的预览窗口:**一个窗口装多个标签**,每个文件一个标签(照 [`crate::settings_window`]
//! 的模式 —— 独立顶层窗口、自绘标题栏、外层包 gpui-kit `Root`)。
//!
//! ## 为什么是独立窗口
//!
//! 预览正文(编辑器 / 图片)与终端是两种东西:塞进 dock 就得把
//! `terminal_panel::TabPane` 从「一堆终端」泛化成「一堆视图」,而终端那条线上每个假设
//! (`focused_view().read(cx).remote_fs()`、标签名取自终端标题、格子布局与焦点…)都得改。
//! 独立窗口对现有代码**零改动**,而且大图 / 长文本本来就该有一块自己的地方。
//!
//! ## 看的是本地缓存,文本能改回去
//!
//! 远端文件由文件管理器**先下到本地缓存**(见 [`crate::sidebar_panel::files`])再看。这样
//! 文本 / 图片都只需要认识一个本地路径(图片走 gpui 的图片元素),不必为「远端」再各写一套
//! 解码。窗口关掉时把缓存文件删掉。
//!
//! **文本是一个编辑器**:行号 + 语法高亮(语言按扩展名认,见 [`Language`])+ 可改。
//! Ctrl+S(或工具条那枚「保存」)把编辑器里那份快照写回**远端**(见 [`PreviewWindow::save`]);
//! 还没写回就关窗口会先问一句(见 [`PreviewWindow::should_close`])。图片只看不动。
//!
//! ## 一个窗口,多个标签
//!
//! 打开多个文件时**只开一个预览窗口**,每个文件一个标签(照普通桌面编辑器):自绘标题栏里
//! 只有窗口控制按钮,**不显示当前文件名** —— 文件名在标签条与状态栏上。
//! 文件管理器「打开」第二个文件时,[`AppRoot::open_preview_window`] 发现窗口已开,就把请求塞
//! 进去加一个标签(见 [`PreviewWindow::add_request`]),不再另开窗口。
//!
//! ## 窗口的几层(照普通桌面文本编辑器的摆法)
//!
//! ```text
//! 自绘标题栏   窗口控制(右端)
//! 工具条       保存 关闭 | 撤销 重做 | 剪切 复制 粘贴 | 查找 替换 跳转行 | 自动换行
//! 标签条       文件名 ●  × | 文件名 ●  × | …
//! 正文         编辑器 / 图片 / 占位说明(当前标签的)
//! 状态栏       主机:远端路径  大小  写回状态 … 语言 | 编码 | Row·Col·Pos | 行数·字符数
//! ```
//!
//! - **工具条 / 正文 / 状态栏都作用在当前标签上**:工具条里那些编辑动作派发到当前标签的
//!   编辑器。⚠️ 「保存 / 关闭 / 撤销…」那一排只按**当前标签**是不是文本来摆(图片 / 其他
//!   没有编辑器,摆了也没东西可编辑)。
//! - **工具条里那些编辑动作不是本窗口实现的**(撤销 / 重做 / 剪切 / 复制 / 粘贴 / 查找 /
//!   替换):它们派发 gpui-base 编辑器自带的 action,由编辑器自己接(见 [`action_button`])。
//!   窗口级那几个动作(保存 / 关闭 / 自动换行 / 跳转行)才由本窗口实现,
//!   注册在 `actions.rs`。
//!
//! ## 什么在应用内、什么交给系统
//!
//! | 类型 | 界面 |
//! |---|---|
//! | 文本 | **编辑器**(行号 + 语法高亮 + 可改):工具条上是标准编辑动作,Ctrl+S 写回远端;超过 [`MAX_TEXT_BYTES`] 或不是 UTF-8 就不加载 |
//! | 图片 | `img` 元素等比缩小放进窗口(`ObjectFit::ScaleDown`,小图不放大) |
//! | 其他 | 占位说明(没有内嵌预览的类型) |
//!
//! ⚠️ **音频 / 视频不在这里** —— 进程里没有解码器(内嵌播放要么用 Windows Media Foundation
//! 自己写一层 unsafe 的 COM,要么引来 ffmpeg),所以由文件管理器下完缓存**直接交给系统默认
//! 程序**(见 [`crate::sidebar_panel::files`] 的 `open_file`);这个窗口收不到这两种类型。

use std::path::{Path, PathBuf};

use futures::StreamExt as _;
use gpui::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _, Render,
    SharedString, Styled as _, StyledImage as _, Subscription, WeakEntity, Window, WindowBounds,
    WindowKind, WindowOptions, div, img, px, size,
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Root, Selectable as _, Sizable as _, TitleBar,
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    h_flex,
    input::{
        Copy, Cut, Editor, EditorState, Input, InputEvent, InputState, Paste, Position, Redo,
        Replace, Rope, RopeExt as _, Search, Undo,
    },
    notification::Notification,
    tab::{Tab, TabBar},
    v_flex,
};
use terminal::{SshFs, TransferEvent};

use crate::AppRoot;
use crate::actions::{
    ClosePreviewWindow, GotoPreviewLine, SavePreviewFile,
    TogglePreviewWordWrap,
};
use crate::assets::IconName;
use crate::format;
use crate::keymap::PREVIEW_CONTEXT;
use crate::status_bar::{STATUS_BAR_ITEM_HEIGHT, new_status_bar};
/// 窗口初始尺寸(宽, 高)。
const WINDOW_SIZE: (f32, f32) = (860., 620.);
/// 窗口最小尺寸(宽, 高)。
///
/// 宽度容得下工具条那一排按钮(十来枚),再窄文件名就只能缩没了 —— 按钮是 `flex_none` 的,
/// 不会跟着缩。
const MIN_WINDOW_SIZE: (f32, f32) = (620., 320.);
/// 工具条高度(图标那一行)。
const TOOLBAR_HEIGHT: gpui::Pixels = px(36.);
/// 文本预览最多加载多少字节。
///
/// 编辑器的语法解析 / 布局是**全量**的,几十 MB 的日志挂上去就动不了了;超了就不加载,
/// 改成「太大了」的提示(见 [`Body::Failed`])。
const MAX_TEXT_BYTES: u64 = 8 * 1024 * 1024;

/// 预览窗口要显示的东西。
#[derive(Clone)]
pub(crate) struct PreviewRequest {
    /// 显示名(远端文件名)。
    pub(crate) name: SharedString,
    /// 远端主机名 —— 状态栏左侧那条「主机:路径」用(见 [`PreviewWindow::render_status_bar`])。
    pub(crate) host: String,
    /// 远端路径(文本保存时写回的就是它)。
    pub(crate) remote_path: String,
    /// 这条预览所属的连接实例 —— 不同连接的同名路径是两个标签。
    pub(crate) session: terminal::SessionId,
    /// 已经下到本地的缓存文件。
    pub(crate) local_path: PathBuf,
    /// 缓存文件的租约：标签活着就一直持有，关闭时自动删（见 [`crate::preview_cache`]）。
    pub(crate) _lease: crate::preview_cache::CacheLease,
    /// 字节数(远端报的;0 = 不知道)。
    pub(crate) size: u64,
    /// 远端文件系统句柄 —— 文本保存写回远端用(别的类型用不上)。
    pub(crate) fs: SshFs,
    /// 保存成功后让文件管理器把所在目录重读一次(见 [`PreviewWindow::refresh_parent_dir`])。
    pub(crate) app_root: WeakEntity<AppRoot>,
}

/// 按扩展名分的类型。
///
/// 两个用途:**走哪条路打开**(文本 / 图片开预览窗口,音频 / 视频交给系统,其余进占位窗口)
/// 与**工具条上怎么标**。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FileKind {
    Text,
    Image,
    Audio,
    Video,
    Other,
}

impl FileKind {
    /// 要不要**跳过预览窗口**、由文件管理器下完缓存直接交给系统默认程序打开。
    ///
    /// 只有音频 / 视频走这条(应用里没有解码器,见模块文档);判定放在这里是因为分类清单只此一份
    /// (见 [`Self::of`]),调用方在 [`crate::sidebar_panel::files`] 的 `open_file`。
    pub(crate) fn handed_to_system(self) -> bool {
        matches!(self, Self::Audio | Self::Video)
    }

    /// 按文件名(扩展名)判断。⚠️ 图片那一条**直接用** gpui 图片元素的扩展名清单
    /// ([`gpui::Img::extensions`]),免得这里的清单和它支持的解码器悄悄走散。
    pub(crate) fn of(name: &str) -> Self {
        let lower = name.to_ascii_lowercase();
        let extension = lower.rsplit_once('.').map(|(_, extension)| extension);
        if let Some(extension) = extension {
            if gpui::Img::extensions().contains(&extension) {
                return Self::Image;
            }
            if matches!(
                extension,
                "mp3" | "wav" | "flac" | "ogg" | "oga" | "opus" | "m4a" | "aac" | "wma" | "aiff"
            ) {
                return Self::Audio;
            }
            if matches!(
                extension,
                "mp4" | "mkv" | "webm" | "avi" | "mov" | "wmv" | "flv" | "m4v" | "mpg" | "mpeg"
                    | "ts" | "3gp"
            ) {
                return Self::Video;
            }
            if TEXT_EXTENSIONS.contains(&extension) {
                return Self::Text;
            }
            return Self::Other;
        }
        // 没有扩展名的:认那几个常见的纯文本文件名,其余当「其他」。
        if matches!(
            lower.as_str(),
            "makefile" | "dockerfile" | "license" | "readme" | "changelog" | "gitignore"
        ) {
            Self::Text
        } else {
            Self::Other
        }
    }

    /// 工具条上的类型名。
    fn label(self) -> &'static str {
        match self {
            Self::Text => "文本",
            Self::Image => "图片",
            Self::Audio => "音频",
            Self::Video => "视频",
            Self::Other => "文件",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::Text => IconName::FileText,
            Self::Image => IconName::Image,
            Self::Audio => IconName::Music,
            Self::Video => IconName::Video,
            Self::Other => IconName::File,
        }
    }
}

/// 「这个文件算不算文本」的扩展名(决定 [`FileKind::Text`],与具体染哪种颜色无关 ——
/// 那件事归 [`Language`])。宽一点没关系:多认一种文本只是多一个能看的文件。
const TEXT_EXTENSIONS: [&str; 62] = [
    "txt", "text", "md", "markdown", "rst", "log", "csv", "tsv", "json", "jsonc", "json5", "toml",
    "yaml", "yml", "ini", "cfg", "conf", "properties", "env", "xml", "html", "htm", "css", "scss",
    "less", "js", "mjs", "cjs", "ts", "tsx", "jsx", "vue", "svelte", "rs", "py", "pyi", "rb", "go",
    "c", "h", "cc", "cpp", "cxx", "hpp", "hxx", "java", "kt", "kts", "swift", "php", "pl", "lua",
    "sh", "bash", "zsh", "fish", "ps1", "bat", "cmd", "sql", "diff", "patch",
];

/// 预览编辑器认得的语言。
///
/// ⚠️ 每一条都要在 `Cargo.toml` 里开对应的 `tree-sitter-*` feature(那张清单与这份枚举是
/// 一对):没开的话 gpui-kit 的注册表里没有这个语法,[`Self::id`] 就只是个没有高亮的普通
/// 模式 —— 不会报错,只是不染色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Language {
    /// 纯文本:没有语法(`.txt` / 认不出来的扩展名都走这里)。
    Plain,
    /// shell 脚本(`.sh` / `.bash` / …):用 bash 语法。
    Shell,
    Python,
    Json,
    Yaml,
    Toml,
    Rust,
    JavaScript,
    TypeScript,
    /// `.tsx`:语法树和 `.ts` 不是同一棵,单列一条(显示名仍旧写 TypeScript)。
    Tsx,
    C,
    Cpp,
    Markdown,
}

impl Language {
    /// 按文件名(扩展名)判断。⚠️ 认不出来的一律 [`Self::Plain`](不硬套一个语法)。
    fn of(name: &str) -> Self {
        let lower = name.to_ascii_lowercase();
        let Some((_, extension)) = lower.rsplit_once('.') else {
            return Self::Plain;
        };
        match extension {
            "sh" | "bash" | "zsh" | "ksh" | "fish" => Self::Shell,
            "py" | "pyi" | "pyw" => Self::Python,
            "json" | "jsonc" => Self::Json,
            "yaml" | "yml" => Self::Yaml,
            "toml" => Self::Toml,
            "rs" => Self::Rust,
            "js" | "mjs" | "cjs" | "jsx" => Self::JavaScript,
            "ts" | "mts" | "cts" => Self::TypeScript,
            "tsx" => Self::Tsx,
            "c" | "h" => Self::C,
            "cc" | "cpp" | "cxx" | "hpp" | "hxx" | "hh" | "h++" | "inl" => Self::Cpp,
            "md" | "markdown" => Self::Markdown,
            _ => Self::Plain,
        }
    }

    /// 交给编辑器的语言名(gpui-kit 的注册表认这些 id)。
    fn id(self) -> &'static str {
        match self {
            Self::Plain => "text",
            Self::Shell => "bash",
            Self::Python => "python",
            Self::Json => "json",
            Self::Yaml => "yaml",
            Self::Toml => "toml",
            Self::Rust => "rust",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::Markdown => "markdown",
        }
    }

    /// 状态栏上的显示名(照 VS Code 的叫法)。
    fn label(self) -> &'static str {
        match self {
            Self::Plain => "Plain Text",
            Self::Shell => "Unix Shell Script",
            Self::Python => "Python",
            Self::Json => "JSON",
            Self::Yaml => "YAML",
            Self::Toml => "TOML",
            Self::Rust => "Rust",
            Self::JavaScript => "JavaScript",
            Self::TypeScript | Self::Tsx => "TypeScript",
            Self::C => "C",
            Self::Cpp => "C++",
            Self::Markdown => "Markdown",
        }
    }
}

/// 读回来的文本 + 它在磁盘上的编码。
struct Loaded {
    text: String,
    /// 头三个字节是 BOM(它**留在**文本里 ⇒ 存回去时也带着;状态栏写 `UTF-8 with BOM`)。
    bom: bool,
}

impl Loaded {
    /// 状态栏上的编码名。
    fn encoding(&self) -> &'static str {
        if self.bom {
            "UTF-8 with BOM"
        } else {
            "UTF-8"
        }
    }
}

/// 光标在哪:`(行, 列, 整篇里的字符位置)`,**都从 1 起**(照编辑器的习惯)。
///
/// `列` 按**字符**算(一个汉字占一列),不是字节。
fn caret(text: &Rope, offset: usize) -> (usize, usize, usize) {
    // `RopeExt::offset_to_position` 自己会把越界 / 落在半个字符里的偏移夹回来,给的是
    // 0 起的行 + **字符**列。
    let position = text.offset_to_position(offset);
    let (row, column) = (position.line as usize, position.character as usize);
    // 整篇的字符位置 = 这一行开头的字符下标 + 行内列(`line_start_offset` 的行口径也是 LF,
    // 与上面那行号一致,见 [`counts`])。
    let chars = text.byte_to_char_idx(text.line_start_offset(row));
    (row + 1, column + 1, chars + column + 1)
}

/// 整篇有多少行 / 多少字符(状态栏右边那两个数)。
fn counts(text: &Rope) -> (usize, usize) {
    (text.lines_len(), text.len_chars())
}


/// 正文现在是什么。
enum Body {
    /// 还在读(只有文本会走这一步)。
    Loading,
    /// 编辑器 + 写回远端那本账(见 [`TextEditing`])。
    Text(TextEditing),
    /// 图片:交给 `img` 元素读本地那份。
    Image,
    /// 没有内嵌预览的类型:只摆一句说明(音频 / 视频不会走到这里,它们由文件管理器直接交给
    /// 系统默认程序,见模块文档)。
    External(&'static str),
    /// 打不开(太大 / 不是文本 / 本地读失败)。
    Failed(String),
}

/// 「文本」正文:编辑器 + 写回远端那本账。
struct TextEditing {
    editor: Entity<EditorState>,
    /// 状态栏上标的语言(见 [`Language`])。
    language: Language,
    /// 状态栏上标的编码。
    encoding: &'static str,
    /// 自动换行开着没有(工具条那枚开关的当前值)。
    ///
    /// 本窗口得自己记着:gpui-base 的编辑器只在构造时收这个开关(`set_soft_wrap`),没有读回来的
    /// 方法。⚠️ 初值必须与编辑器的默认值一致(它的 `soft_wrap` 默认 **true**),否则开关的显示
    /// 状态会和编辑器里的实际行为对不上。
    soft_wrap: bool,
    /// 改动 / 写回到哪一版了。
    revisions: Revisions,
    /// 正在写回的那一次;`None` = 没在写。
    saving: Option<Saving>,
    /// 最近一次写回的结果(工具条上那行小字)。
    outcome: Option<Result<(), String>>,
    /// 编辑器的订阅:改动记版本 + 光标动了要刷新状态栏(状态栏那几个数是每帧现读的,
    /// 见 [`PreviewWindow::render_status_bar`])。
    _subs: Vec<Subscription>,
}

impl TextEditing {
    fn is_dirty(&self) -> bool {
        self.revisions.is_dirty()
    }

    fn is_saving(&self) -> bool {
        self.saving.is_some()
    }
}

/// 「改动到哪一版 / 写回到哪一版」的账本。
///
/// 分成两个数是因为**写回是异步的**:开写时抓的是某一版,路上用户还能接着改 ——
/// 回来后不能把「已写回」记到**最新**那一版上,否则那句「已写回」会盖住还没写回的改动
/// (见 [`Revisions::mark_saved`] 的调用处)。
#[derive(Default)]
struct Revisions {
    /// 编辑器每改一次 +1。
    current: u64,
    /// 最近一次**成功**写回远端的版本。
    saved: u64,
}

impl Revisions {
    /// 编辑器改动了一笔。
    fn edited(&mut self) {
        self.current += 1;
    }

    /// 记下「**这一版**已经写回远端」。⚠️ 传的必须是开写时那份快照的版本号,不是 `current`。
    fn mark_saved(&mut self, revision: u64) {
        self.saved = revision;
    }

    fn is_dirty(&self) -> bool {
        self.current != self.saved
    }
}

/// 一次在跑的写回。
struct Saving {
    id: SaveOperationId,
    /// 抓那份文本时的版本号(见 [`Revisions`])。
    revision: u64,
    /// 已经写过去多少字节。
    done: u64,
    /// 这次要写多少字节(工具条上那行进度小字的分母)。
    total: u64,
}

impl Saving {
    fn matches(saving: Option<&Self>, operation_id: SaveOperationId) -> bool {
        saving.is_some_and(|saving| saving.id == operation_id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SaveOperationId(u64);

impl SaveOperationId {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        Self(NEXT.fetch_update(
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
            |value| value.checked_add(1),
        ).expect("save operation identity exhausted"))
    }
}

/// 关窗口时该怎么办(见 [`PreviewWindow::should_close`])。
#[derive(Clone, Copy, PartialEq, Eq)]
enum CloseAsk {
    /// 放行。
    Allow,
    /// 正在写回:挡下来。
    Saving,
    /// 有没写回的改动:弹确认框。
    Confirm,
}

/// 窗口内那些「关闭」入口该怎么办(见 [`PreviewWindow::close_decide`])。
///
/// 与 [`CloseAsk`] 差一个「框要不要弹」:`Confirm` 带上**当前标签**的文件名,弹框那一步由调用
/// 方做 —— 判定要能就地做(`&self`),而 `open_unsaved_dialog` 要 `WeakEntity`。
enum Decision {
    /// 真的关掉(当前标签;它又是最后一个 ⇒ 连窗口一起收)。
    Close,
    /// 挡下来(写回中 / 已经有个框开着)。
    Blocked,
    /// 弹「还有没写回的改动」确认框。
    Confirm(SharedString),
}

/// 「有没写回的改动」那个确认框是替谁问的(见 [`open_unsaved_dialog`])。
#[derive(Clone, Copy)]
enum UnsavedScope {
    /// 关某个标签(`丢弃` / `保存` 只作用于它)。
    Tab(usize),
    /// 关整个窗口(`丢弃` 关窗;`保存全部` 把所有脏标签写回,写完再关)。
    Window,
}

/// 一个标签:一个文件,连同它现在读到哪了、改到哪了。
///
/// 除了「窗口认得的」(焦点句柄 / 主窗口订阅)都挂在这里 —— 一个窗口开多个文件时,每个文件
/// 各有各的正文与写回账本。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PreviewId(u64);

impl PreviewId {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        Self(NEXT.fetch_update(
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
            |value| value.checked_add(1),
        ).expect("preview identity exhausted"))
    }
}

pub(crate) struct PreviewTab {
    id: PreviewId,
    /// 打开它时的那些信息(远端路径 / 主窗口句柄 / 缓存文件…)。
    request: PreviewRequest,
    kind: FileKind,
    body: Body,
    /// 「保存并关闭」:关这个标签那个确认框里点了「保存」时为真,写回成功了这个标签才真关。
    close_after_save: bool,
}

/// 预览窗口(**一个窗口装多个标签**)。
pub(crate) struct PreviewWindow {
    /// 所有标签,按打开顺序。
    tabs: Vec<PreviewTab>,
    /// 当前标签下标。
    active: usize,
    /// 主窗口;它关了就退出应用(照 [`crate::settings_window::SettingsWindow`])。
    _main_window_closed: Subscription,
    /// 窗口根节点的焦点句柄(`track_focus` 用,也是 [`Focusable`] 给的本窗口焦点)。
    ///
    /// ⚠️ 它不主动抢焦点,编辑器的焦点照旧(见 [`PreviewWindow::focus_active`])。
    root_focus: FocusHandle,
}

/// 建一个标签:按类型定初始正文,文本的还要起后台读。
///
/// 返回建好的标签,**不**往窗口里放(调用方决定放哪 / 是不是当前标签)。
fn new_tab(
    request: PreviewRequest,
    window: &mut Window,
    cx: &mut Context<PreviewWindow>,
) -> PreviewTab {
    let kind = FileKind::of(&request.name);
    let body = match kind {
        FileKind::Text => Body::Loading,
        FileKind::Image => Body::Image,
        // 音频 / 视频由文件管理器直接交给系统默认程序,到不了这个窗口(见 [`FileKind::handed_to_system`])。
        FileKind::Audio | FileKind::Video | FileKind::Other => {
            Body::External("这个类型没有内嵌预览。")
        }
    };
    let mut tab = PreviewTab {
        id: PreviewId::new(),
        request,
        kind,
        body,
        close_after_save: false,
    };
    if kind == FileKind::Text {
        load_text(&mut tab, window, cx);
    }
    tab
}

/// 读本地缓存那份文本,读回来再建编辑器(建 [`EditorState`] 要窗口,所以只能在 UI 侧做)。
fn load_text(tab: &mut PreviewTab, window: &mut Window, cx: &mut Context<PreviewWindow>) {
    let path = tab.request.local_path.clone();
    let preview_id = tab.id;
    cx.spawn_in(
        window,
        async move |this: WeakEntity<PreviewWindow>, cx: &mut gpui::AsyncWindowContext| {
            let loaded = cx
                .background_executor()
                .spawn(async move { read_text(&path) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_load(preview_id, loaded, window, cx)
            });
        },
    )
    .detach();
}

impl PreviewWindow {
    fn new(
        request: PreviewRequest,
        main_window: AnyWindowHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let main_window_id = main_window.window_id();
        let _main_window_closed = cx.on_window_closed(move |cx, closed| {
            if closed == main_window_id {
                cx.quit();
            }
        });
        let tab = new_tab(request, window, cx);
        let mut this = Self {
            tabs: vec![tab],
            active: 0,
            _main_window_closed,
            root_focus: cx.focus_handle(),
        };
        // 关窗口前拦一道:有没写回的改动时弹确认框(见 [`Self::should_close`])。
        let guard = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| Self::should_close(&guard, window, cx));
        // 首标签是文本的话把焦点给它的编辑器(打开就能直接敲)。
        this.focus_active(window, cx);
        this
    }

    /// 当前标签;窗口里一个标签都没有时 `None`(只在最后那个标签关闭的瞬间会出现)。
    fn active(&self) -> Option<&PreviewTab> {
        self.tabs.get(self.active)
    }

    fn active_mut(&mut self) -> Option<&mut PreviewTab> {
        self.tabs.get_mut(self.active)
    }

    /// 把一个新请求加成标签(已存在同一个远端路径的直接切过去,不重复开)。
    ///
    /// 给文件管理器「打开」第二个文件时用(见 [`AppRoot::open_preview_window`]):窗口已经开着,
    /// 不再另开窗口,而是往这里加一枚标签。
    pub(crate) fn add_request(
        &mut self,
        request: PreviewRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ix) = self
            .tabs
            .iter()
            .position(|tab| {
                tab.request.session == request.session && tab.request.remote_path == request.remote_path
            })
        {
            // 同一个远端文件已经开着:切过去就行(内容可能已变，但用户看得见标签就在那儿)。
            // 重复请求那份新缓存随 `request` 析构，租约释放时自动删。
            self.active = ix;
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        let tab = new_tab(request, window, cx);
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        self.focus_active(window, cx);
        cx.notify();
    }

    /// 把焦点交给当前标签的编辑器(不是文本就没有编辑器,不动焦点 —— 照原来「图片 / 其他
    /// 不抢焦点」的行为)。
    fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 先取句柄、再动窗口(`cx` 要借出去)。
        let Some(focus) = self.active().and_then(|tab| editor_focus(tab, cx)) else {
            return;
        };
        window.focus(&focus, cx);
    }

    /// 切换当前标签(标签条点某一枚时用)。
    fn select_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() || ix == self.active {
            return;
        }
        self.active = ix;
        self.focus_active(window, cx);
        cx.notify();
    }

    /// 关掉第 `ix` 个标签;关的是最后一个 ⇒ 连窗口一起收。
    ///
    /// ⚠️ 收尾**必须推迟一帧**:此刻本视图正被 `update`(listener 握着 lease),就地
    /// `remove_window` 会在窗口销毁 / 视图再读时撞上 gpui 的双重租借检查。
    fn close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        if self.tabs.len() == 1 {
            window.defer(cx, |window, _cx| window.remove_window());
            return;
        }
        let tab = self.tabs.remove(ix);
        // 标签没了 ⇒ 它那份缓存租约随 `tab` 析构释放（最后一份时删文件）。
        drop(tab);
        if self.active > ix {
            self.active -= 1;
        } else if self.active == ix {
            self.active = self.active.min(self.tabs.len() - 1);
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    /// 工具条 / 标签上那两枚「关闭」:关**当前标签**;关的是最后一个 ⇒ 连窗口一起收。
    ///
    /// ⚠️ 判定**就地**做(`&self`),不再回过头去 `read` 本实体 —— 见 [`Self::close_gate`]
    /// 那段关于双重租借的说明。
    fn request_close_self(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        match self.close_decide(window, cx) {
            Decision::Close => self.close_tab(self.active, window, cx),
            Decision::Blocked => {}
            Decision::Confirm(name) => {
                open_unsaved_dialog(weak, UnsavedScope::Tab(self.active), name, window, cx)
            }
        }
    }

    /// 标签条上某一枚 × :关**它**(不是当前标签的那个也在)。
    ///
    /// 不是当前标签的先切过去再走同一套判定 —— 判定 / 弹框都作用在当前标签上,逻辑只有一份。
    fn request_close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        if ix != self.active {
            self.select_tab(ix, window, cx);
        }
        self.request_close_self(window, cx);
    }

    /// 关窗口该不该放行(`on_window_should_close` 的处理器,见 [`Self::new`])。
    ///
    /// 判定本身在 [`Self::close_gate`] —— 标题栏那个 X 与窗口内的「关闭」入口(工具条 /
    /// 标签上的 ×,见 [`Self::request_close_self`])走的都是同一份「按标签问」判定,只有
    /// 「放行之后谁收尾」不同:这条路上**返回 `false` 挡下系统那次关闭**是**故意**的,放行那支
    /// 返回 `true`,让平台照它自己的流程关(不要去 `remove_window()` —— 那是在平台的关闭流程里
    /// 再插一脚)。
    fn should_close(weak: &WeakEntity<Self>, window: &mut Window, cx: &mut App) -> bool {
        Self::close_gate(weak, window, cx)
    }

    /// 关窗口前那道闸:`true` = 可以真的关掉。
    ///
    /// 关窗口一道闸要把**所有标签**都过一遍:任意一个正在写回 ⇒ 挡一下(写了一半就断,远端那份
    /// 可能是残的);任意一个有没写回的改动 ⇒ 弹一次确认框(保存全部 / 丢弃 / 取消)并挡下 ——
    /// 用户选了「丢弃」或全部写回成功后再由那边自己把窗口收掉。
    ///
    /// ⚠️ 这条路上**读本实体是对的**:它是平台的 `on_window_should_close` 回调,不在任何
    /// `update` 里跑,所以 `read_with` 不会撞上双重租借。(窗口内那几个入口另走
    /// [`Self::close_decide`],理由见那里。)
    fn close_gate(weak: &WeakEntity<Self>, window: &mut Window, cx: &mut App) -> bool {
        // 视图已经没了(窗口在收尾)⇒ 没什么可拦的。
        let Ok((saving, dirty_count)) = weak.read_with(cx, |this, _| {
            let saving = this.tabs.iter().any(|tab| close_ask(tab) == CloseAsk::Saving);
            let dirty = this
                .tabs
                .iter()
                .filter(|tab| close_ask(tab) == CloseAsk::Confirm)
                .count();
            (saving, dirty)
        }) else {
            return true;
        };
        if saving {
            window.push_notification(Notification::warning("正在写回远端,写完才能关窗口"), cx);
            return false;
        }
        if dirty_count > 0 {
            // 点两次 X 不该叠两个框。
            if !window.has_active_dialog(cx) {
                let name = if dirty_count > 1 {
                    format!("{dirty_count} 个文件")
                } else {
                    weak.read_with(cx, |this, _| {
                        this.tabs
                            .iter()
                            .find(|tab| close_ask(tab) == CloseAsk::Confirm)
                            .map(|tab| tab.request.name.to_string())
                            .unwrap_or_default()
                    })
                    .unwrap_or_default()
                };
                open_unsaved_dialog(weak.clone(), UnsavedScope::Window, name.into(), window, cx);
            }
            return false;
        }
        true
    }

    /// 关**当前标签**那个判定的**就地面**(`&self`),给窗口内的入口用。
    ///
    /// ⚠️ 为什么不让这些入口复用 [`Self::close_gate`]:它开头要 `weak.read_with(cx, ..)` 读
    /// 本实体,而这些入口**全部**是从 `Context<PreviewWindow>` 的 listener 里进来的 ——
    /// 那一刻实体正被 `update`(listener 握着 lease),再 `read` 一次就是 gpui 的
    /// `cannot read PreviewWindow while it is already being updated` 硬崩
    /// (`entity_map.rs` 的双重租借检查,`&self` / `&mut self` 方法里的 `cx` 已经能直接看
    /// 字段,不需要也不该再读一遍)。
    fn close_decide(&self, window: &mut Window, cx: &mut App) -> Decision {
        let Some(tab) = self.active() else {
            return Decision::Close;
        };
        match close_ask(tab) {
            CloseAsk::Allow => Decision::Close,
            CloseAsk::Saving => {
                window.push_notification(Notification::warning("正在写回远端,写完才能关窗口"), cx);
                Decision::Blocked
            }
            CloseAsk::Confirm => {
                // 点两次 X 不该叠两个框。
                if window.has_active_dialog(cx) {
                    Decision::Blocked
                } else {
                    Decision::Confirm(tab.request.name.clone())
                }
            }
        }
    }

    /// 后台把本地缓存读回来之后收尾，按标签身份定位。
    fn finish_load(
        &mut self,
        preview_id: PreviewId,
        loaded: Result<Loaded, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self
            .tabs
            .iter()
            .position(|tab| tab.id == preview_id)
        else {
            // 标签在读的中途被关了 ⇒ 正文就别摆了。
            return;
        };
        show_text(&mut self.tabs[ix], loaded, window, cx);
        // 读的正好是当前标签 ⇒ 焦点给它的编辑器(打开就能直接敲)。
        if ix == self.active {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    /// 编辑器改动:记一笔版本号,并把上一次「已写回 / 失败」的小字清掉(它已经过期了)。
    fn on_editor_event(
        &mut self,
        editor: Entity<EditorState>,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        if !matches!(event, InputEvent::Change) {
            return;
        }
        // 按编辑器句柄找回它那个标签(不是当前标签的那个也在改)。
        let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|tab| matches!(&tab.body, Body::Text(text) if text.editor == editor))
        else {
            return;
        };
        let Body::Text(text) = &mut tab.body else {
            return;
        };
        text.revisions.edited();
        text.outcome = None;
        cx.notify();
    }

    /// Ctrl+S / 工具条「保存」:把**当前标签**编辑器里那份**快照**写回远端(见 [`Self::save_tab`])。
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_tab(self.active, window, cx);
    }

    /// 把第 `ix` 个标签编辑器里那份**快照**写回远端。
    ///
    /// 一条链:抓快照 → 落本地缓存那份(编辑器只活在内存里)→ 交给传输 worker 写远端
    /// (见 [`terminal::SshFs::upload_one`])。路上用户接着改不影响这次写回的内容,写完了也只
    /// 把「已写回」记在**快照那一版**上(见 [`Revisions`])。
    fn save_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(ix) else {
            return;
        };
        let Body::Text(text) = &mut tab.body else {
            return;
        };
        // 没改过就别写(顺带挡掉「打开就按 Ctrl+S」这种空写);上一次还在路上也别叠第二次。
        if !text.is_dirty() || text.is_saving() {
            return;
        }
        let contents = text.editor.read(cx).value().to_string();
        let total = contents.len() as u64;
        let operation_id = SaveOperationId::new();
        text.saving = Some(Saving {
            id: operation_id,
            revision: text.revisions.current,
            done: 0,
            total,
        });
        text.outcome = None;
        let cache = tab.request.local_path.clone();
        let preview_id = tab.id;
        let remote_path = tab.request.remote_path.clone();
        let fs = tab.request.fs.clone();
        cx.notify();

        cx.spawn_in(
            window,
            async move |this: WeakEntity<Self>, cx: &mut gpui::AsyncWindowContext| {
                let cache_path = cache.clone();
                let written = cx
                    .background_executor()
                    .spawn(async move { std::fs::write(&cache_path, contents) })
                    .await;
                if let Err(error) = written {
                    let message = format!("写本地缓存失败:{error}");
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.finish_save(preview_id, operation_id, Err(message), window, cx)
                    });
                    return;
                }
                let mut events = fs.upload_one(cache.clone(), remote_path.clone()).events;
                while let Some(event) = events.next().await {
                    match event {
                        TransferEvent::Progress(progress) => {
                            let (done, total) = (progress.bytes_done, progress.bytes_total);
                            // 视图已经没了也把这次写回读完:半截的远端文件比多等一会儿糟。
                            let _ = this.update_in(cx, |this, window, cx| {
                                this.on_save_progress(preview_id, operation_id, done, total, window, cx)
                            });
                        }
                        TransferEvent::Done(result) => {
                            let result = result.map_err(|error| error.to_string());
                            let _ = this.update_in(cx, |this, window, cx| {
                                this.finish_save(preview_id, operation_id, result, window, cx)
                            });
                            return;
                        }
                    }
                }
                // 通道在 `Done` 之前就断了:worker 线程没了(照 `sidebar_panel/files.rs` 那条路)。
                let _ = this.update_in(cx, |this, window, cx| {
                    this.finish_save(
                        preview_id,
                        operation_id,
                        Err("远端文件系统线程已退出".to_string()),
                        window,
                        cx,
                    )
                });
            },
        )
        .detach();
    }

    /// 写回进度(状态栏那行小字)，按标签身份定位。
    fn on_save_progress(
        &mut self,
        preview_id: PreviewId,
        operation_id: SaveOperationId,
        done: u64,
        total: u64,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == preview_id)
        else {
            return;
        };
        let Body::Text(text) = &mut tab.body else {
            return;
        };
        let Some(saving) = text.saving.as_mut() else {
            return;
        };
        if !Saving::matches(Some(saving), operation_id) {
            return;
        }
        saving.done = done;
        if total > 0 {
            saving.total = total;
        }
        cx.notify();
    }

    /// 一次写回收尾(成功 / 失败)。`result` 里带的是已经翻成人话的失败原因。
    ///
    /// 按标签身份定位；标签已经关闭时丢弃结果。
    fn finish_save(
        &mut self,
        preview_id: PreviewId,
        operation_id: SaveOperationId,
        result: Result<(), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self
            .tabs
            .iter()
            .position(|tab| tab.id == preview_id)
        else {
            return;
        };
        let outcome = {
            let tab = &mut self.tabs[ix];
            let Body::Text(text) = &mut tab.body else {
                return;
            };
            if !Saving::matches(text.saving.as_ref(), operation_id) {
                return;
            }
            let Some(saving) = text.saving.take() else {
                return;
            };
            match result {
                Ok(()) => {
                    text.revisions.mark_saved(saving.revision);
                    text.outcome = Some(Ok(()));
                    Ok(saving.total)
                }
                Err(message) => {
                    text.outcome = Some(Err(message.clone()));
                    Err(message)
                }
            }
        };
        match outcome {
            Ok(size) => {
                // 远端那份变了:状态栏上的大小跟着变,文件管理器那边把目录重读一次。
                self.tabs[ix].request.size = size;
                refresh_parent_dir(&self.tabs[ix], cx);
                if self.tabs[ix].close_after_save {
                    // 关这个标签(或关窗口那一路的某一个)那一下点的「保存」:写回成功了才真的收。
                    // ⚠️ 关的是**最后一个**标签时 [`Self::close_tab`] 顺手把窗口也收掉(见
                    // [`Self::save_then_close`])。
                    self.close_tab(ix, window, cx);
                }
            }
            Err(message) => {
                // 这次没成 ⇒ 别再自动关,让用户看着错误自己决定。
                self.tabs[ix].close_after_save = false;
                let note = format!("写回远端失败:{message}");
                log::warn!("{}:{note}", self.tabs[ix].request.remote_path);
                window.push_notification(Notification::error(note), cx);
            }
        }
        cx.notify();
    }

    /// 工具条「跳转到行」:弹一个只填行号的小框(见 [`open_goto_line_dialog`])。
    fn open_goto_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.active() else {
            return;
        };
        let Body::Text(text) = &tab.body else {
            return;
        };
        // 连按两次 Ctrl+G 不该叠两个框(与关窗口那个确认框同样的挡法)。
        if window.has_active_dialog(cx) {
            return;
        }
        // 先把编辑器句柄拿出来(`cx` 还要借出去建对话框)。
        let editor = text.editor.clone();
        open_goto_line_dialog(editor, window, cx);
    }

    /// 工具条「自动换行」:在自动换行与横向滚动之间切(作用在当前标签上)。
    fn toggle_soft_wrap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.active_mut() else {
            return;
        };
        let Body::Text(text) = &mut tab.body else {
            return;
        };
        text.soft_wrap = !text.soft_wrap;
        let wrap = text.soft_wrap;
        text.editor
            .update(cx, |state, cx| state.set_soft_wrap(wrap, window, cx));
        cx.notify();
    }

    /// 关标签 / 关窗口时点了确认框的「保存」。
    ///
    /// 作用范围按 `scope` 分两档:
    /// - [`UnsavedScope::Tab`] —— 只写回**那个**标签,写完关它(它是最后一个 ⇒ 连窗口一起收)。
    /// - [`UnsavedScope::Window`] —— 所有**还没写回的**标签都标上「写完就关」,再逐个派发写回
    ///   (它们同时在路上,各自回来关掉自己 —— 最后一个关掉时 [`Self::close_tab`] 顺手把窗口
    ///   收掉;有谁写回失败就**不关**,留在界面上让用户看着错误)。
    fn save_then_close(
        &mut self,
        scope: UnsavedScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match scope {
            UnsavedScope::Tab(ix) => {
                if let Some(tab) = self.tabs.get_mut(ix) {
                    tab.close_after_save = true;
                }
                self.save_tab(ix, window, cx);
            }
            UnsavedScope::Window => {
                let targets: Vec<usize> = self
                    .tabs
                    .iter()
                    .enumerate()
                    .filter(|(_, tab)| close_ask(tab) == CloseAsk::Confirm)
                    .map(|(ix, _)| ix)
                    .collect();
                for ix in targets {
                    self.tabs[ix].close_after_save = true;
                    self.save_tab(ix, window, cx);
                }
                // 一个都没得写(全在路上 / 已写回)⇒ 直接关窗,别卡着。
                if !self.tabs.iter().any(|tab| tab.close_after_save) {
                    window.defer(cx, |window, _cx| window.remove_window());
                }
            }
        }
    }

    /// 工具条:图标按钮,按「文件 / 撤改 / 剪贴 / 查找 / 编辑器」分组,组间一条竖线。
    ///
    /// ⚠️ 除了「保存 / 关闭 / 跳转行 / 自动换行」,其余按钮**不是本窗口实现
    /// 的**:它们只把 gpui-base 编辑器那几个 action(`gpui_kit::component::input`)派发给
    /// **当前聚焦的元素**,由编辑器的动作处理器接下(见 [`action_button`] 与 [`EditorState`]
    /// 渲染里那串 `on_action`)。派发要落到编辑器身上,前提是点按钮不换焦点 —— gpui-kit 的
    /// `Button` 在 `on_mouse_down` 里 `prevent_default()`,正是为此。
    ///
    /// 窗口级那几枚走 `cx.listener` 直接调方法,不绕 action:按钮就在本窗口里,拿得到
    /// `Context<PreviewWindow>`,直说比「派发给当前焦点」少一层不确定性(焦点不在编辑器上时
    /// 派发也会到,但那是运气)。
    ///
    /// ⚠️ 「保存 / 关闭 / 撤销…」那一排**只在当前标签是文本时摆**(图片 / 其他没有编辑器,
    /// 摆了也没东西可编辑):当前标签不是文本时工具条是空的(只剩那条底边)。
    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let text = match self.active().map(|tab| &tab.body) {
            Some(Body::Text(text)) => Some(text),
            _ => None,
        };

        let mut bar = h_flex()
            .h(TOOLBAR_HEIGHT)
            .w_full()
            .flex_none()
            .px_2()
            .gap_0p5()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border);

        if let Some(text) = text {
            bar = bar
                .child(
                    Button::new("preview-save")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Save)
                        .tooltip("保存到远端 (Ctrl+S)")
                        .disabled(text.is_saving() || !text.is_dirty())
                        .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                )
                .child(
                    Button::new("preview-close")
                        .ghost()
                        .xsmall()
                        .icon(IconName::WindowClose)
                        .tooltip("关闭标签 (Ctrl+W)")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.request_close_self(window, cx)),
                        ),
                )
                .child(toolbar_separator(cx))
                .child(action_button("preview-undo", IconName::Undo2, "撤销 (Ctrl+Z)", Undo))
                .child(action_button("preview-redo", IconName::Redo2, "重做 (Ctrl+Y)", Redo))
                .child(toolbar_separator(cx))
                .child(action_button("preview-cut", IconName::Scissors, "剪切 (Ctrl+X)", Cut))
                .child(action_button("preview-copy", IconName::Copy, "复制 (Ctrl+C)", Copy))
                .child(action_button(
                    "preview-paste",
                    IconName::ClipboardPaste,
                    "粘贴 (Ctrl+V)",
                    Paste,
                ))
                .child(toolbar_separator(cx))
                .child(action_button(
                    "preview-find",
                    IconName::Search,
                    "查找 (Ctrl+F)",
                    Search,
                ))
                .child(action_button(
                    "preview-replace",
                    IconName::Replace,
                    "替换 (Ctrl+H)",
                    Replace,
                ))
                .child(
                    Button::new("preview-goto-line")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Hash)
                        .tooltip("跳转到行")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.open_goto_line(window, cx)),
                        ),
                )
                .child(toolbar_separator(cx))
                .child(
                    Button::new("preview-soft-wrap")
                        .ghost()
                        .xsmall()
                        .icon(IconName::TextWrap)
                        .tooltip("自动换行")
                        .selected(text.soft_wrap)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.toggle_soft_wrap(window, cx)),
                        ),
                );
        }

        bar
    }

    /// 标签条:**一个文件一枚标签**(见模块文档),标签上是文件名 + 关闭按钮。
    ///
    /// 用 gpui-kit 的 [`TabBar`] / [`Tab`]:`TabBar::max_width` 会给标签宽度封顶、长名字省略
    /// 中段而**保住后面的关闭按钮**(那两条都有单测盯着),自己搭一套很难不把按钮挤没。
    /// 点某一枚标签 = 切到那个标签(`TabBar::on_click` 给下标);标签上的 × 只关**它自己**
    /// (⚠️ `TabBar::on_click` 会盖掉 `Tab` 自己的 `on_click`,所以 × 的关闭手势由它兜住)。
    fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut tabs = Vec::with_capacity(self.tabs.len());
        for (ix, tab) in self.tabs.iter().enumerate() {
            let dirty = matches!(&tab.body, Body::Text(text) if text.is_dirty());
            let mut built = Tab::new().label(tab.request.name.clone());
            if dirty {
                // 有没写回的改动:名字前面点一颗(状态栏那行小字写的是「写回成功 / 失败」,这里写的是
                // 「还有东西没写回」)。
                built = built.prefix(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(cx.theme().primary)
                        .child("●"),
                );
            }
            // 每一枚 × 只关**它自己**(最后一个也会走 `close_tab`,那里再决定要不要连窗口一起收)。
            // ⚠️ 必须 `stop_propagation`:× 是标签的子节点,点它在冒泡阶段还会撞上标签自己的
            // `on_click`(`TabBar::on_click` 那条「切标签」),不拦就会「先关掉这个标签、又被当成
            // 点了它去切标签」(那时下标已经错位)。
            let close = cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.request_close_tab(ix, window, cx)
            });
            tabs.push(built.suffix(
                Button::new(("preview-tab-close", ix))
                    .ghost()
                    .xsmall()
                    .icon(IconName::WindowClose)
                    .tooltip("关闭标签 (Ctrl+W)")
                    .occlude()
                    .on_click(close),
            ));
        }

        h_flex()
            .w_full()
            .flex_none()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                TabBar::new("preview-tabs")
                    .small()
                    .max_width(px(260.))
                    .selected_index(self.active)
                    .on_click(cx.listener(|this, ix, window, cx| this.select_tab(*ix, window, cx)))
                    .children(tabs),
            )
    }

    /// 正文:当前标签的正文。
    fn render_body(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(tab) = self.active() else {
            return div().size_full().into_any_element();
        };
        match &tab.body {
            Body::Loading => centered(Icon::new(tab.kind.icon()).size_8(), "正在读取…", None, cx),
            Body::Image => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .p_3()
                .child(
                    // `ScaleDown`:等比缩到装得下,小图不放大(预览不该把 16×16 拉成满屏)。
                    img(tab.request.local_path.clone())
                        .size_full()
                        .object_fit(ObjectFit::ScaleDown),
                )
                .into_any_element(),
            // ⚠️ `size_full()` 不能省。多行编辑器没给高度时按**内容行数**撑高,而
            // `LayoutMode::CodeEditor` 的默认 `rows` 是 **2**(见 gpui-kit 的 `mode.rs` 与
            // `input.rs` 里那个 `h_auto()` 分支)⇒ 编辑器会缩成两行、后面的内容全被裁掉,
            // 滚动条还会压在第二行上。官方 story 也是显式 `.size_full()` 才铺满的。
            // 这里**不给** `.readonly(..)`:文本是能改的,改完 Ctrl+S 写回远端(见 `Self::save`)。
            Body::Text(text) => Editor::new(&text.editor).size_full().into_any_element(),
            Body::External(note) => centered(
                Icon::new(tab.kind.icon()).size_8(),
                tab.request.name.as_ref(),
                Some(note),
                cx,
            ),
            Body::Failed(message) => centered(
                Icon::new(IconName::TriangleAlert).size_8(),
                "打不开这个文件",
                Some(message.as_str()),
                cx,
            ),
        }
    }

    /// 底部状态栏:左边是「主机:远端路径」(+ 大小 + 写回状态),右边是语言 / 编码 /
    /// 光标位置 / 行数字符数(照普通文本编辑器的摆法)。都是**当前标签**的。
    ///
    /// 与主窗口共用状态栏布局和组件样式。右侧信息每帧从编辑器读取，光标移动时编辑器会
    /// `cx.notify()`(订阅见 [`PreviewWindow::show_text`]),所以它们跟着光标走。
    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(tab) = self.active() else {
            return new_status_bar()
                .child(div().h(STATUS_BAR_ITEM_HEIGHT))
                .into_any_element();
        };
        // 主机名来自建会话时的参数(见 [`PreviewRequest::host`])——远端那份到底是哪台机器上的,
        // 只有这一处说。大小与写回状态都挨着它摆(⚠️ `StatusBar` 的 `left` / `right` 是两端
        // 各自一条,`child` 才是中间那段;放在中间那段的元素才有地方省略,所以整簇走 `child`)。
        let mut left = h_flex()
            .min_h(STATUS_BAR_ITEM_HEIGHT)
            .flex_1()
            .min_w_0()
            .gap_2()
            .items_center()
            .child(
                Icon::new(IconName::Globe)
                    .size_3p5()
                    .flex_none()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(format!("{}:{}", tab.request.host, tab.request.remote_path)),
            );
        if tab.request.size > 0 {
            left = left.child(div().flex_none().child(format::size(tab.request.size)));
        }
        if let Body::Text(text) = &tab.body {
            // 写回状态:「写回中 42%」/「已写回远端」/「写回失败…」。
            if let Some((label, failed)) = save_label(text) {
                left = left.child(
                    div()
                        .flex_none()
                        .text_color(if failed {
                            cx.theme().danger
                        } else {
                            cx.theme().muted_foreground
                        })
                        .child(label),
                );
            }
        }

        let mut bar = new_status_bar().child(left);

        match &tab.body {
            Body::Text(text) => {
                let state = text.editor.read(cx);
                let (row, col, pos) = caret(state.text(), state.cursor());
                let (lines, chars) = counts(state.text());
                bar = bar
                    .right(text.language.label())
                    .right(text.encoding)
                    .right(format!("Row: {row} | Col: {col} | Pos: {pos}"))
                    .right(format!("{lines} lines | {chars} chars"));
            }
            // 没有编辑器的:至少把类型标出来(大小在左边那簇里)。
            _ => {
                bar = bar.right(tab.kind.label());
            }
        }
        bar.into_any_element()
    }
}

impl Focusable for PreviewWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.root_focus.clone()
    }
}

impl Render for PreviewWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            // 窗口级动作全绑在 keymap 的 `"Preview"` context 上(见 `crate::keymap` 与
            // `assets/keymaps/default-*.json`)。⚠️ 这些动作**只由本窗口自己接**:它们是窗口级的
            // (预览窗口只有一个,里面有多少标签由本窗口自己管),不能像别的动作那样丢给
            // `AppRoot` 的全局监听器 —— 全局那侧拿不到「是哪个预览窗口」。
            .key_context(PREVIEW_CONTEXT)
            .track_focus(&self.root_focus)
            .on_action(cx.listener(|this, _: &SavePreviewFile, window, cx| this.save(window, cx)))
            .on_action(
                cx.listener(|this, _: &ClosePreviewWindow, window, cx| {
                    this.request_close_self(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &GotoPreviewLine, window, cx| {
                this.open_goto_line(window, cx)
            }))
            .on_action(cx.listener(|this, _: &TogglePreviewWordWrap, window, cx| {
                this.toggle_soft_wrap(window, cx)
            }))
            // 顶部:自绘标题栏 —— **只有窗口控制按钮**(`TitleBar` 自己给);不放文件名,也不再
            // 嵌菜单栏(见模块文档):当前文件叫什么在标签条与状态栏上,窗口标题由
            // `set_window_title` 另设。
            .child(TitleBar::new())
            .child(self.render_toolbar(cx))
            .child(self.render_tab_bar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(self.render_body(cx)),
            )
            .child(self.render_status_bar(cx))
            // 覆盖层由 gpui-kit `Root` 渲染。
    }
}

impl Drop for PreviewWindow {
    fn drop(&mut self) {
        // 每个标签都持有缓存租约，`self.tabs` 析构时自动释放（最后一份时删文件）。
        // ⚠️ 有没写回的改动时关窗口那一下先被 [`PreviewWindow::should_close`] 拦下来问了,
        // 所以走到这里的内容要么已经写回远端、要么是用户明确选了「丢弃」。
    }
}

/// 弹「还有没写回的改动」确认框:保存 / 丢弃 / 取消(关标签 / 关窗口那一道,见
/// [`PreviewWindow::request_close_self`] 与 [`PreviewWindow::should_close`])。
///
/// 三个按钮各自收尾,不再走 `Dialog` 的 ok / cancel 那一对回调 —— 那对只有两个通道,分不出
/// 三个选择。`escape` / 点遮罩 = 取消(什么都不做,只是把框收掉)。
///
/// `scope` 说这框是替谁问的:关**某个标签** ⇒ 丢弃 / 保存只作用于它;关**窗口** ⇒ 丢弃直接关窗、
/// 保存把**所有**脏标签都写回(见 [`UnsavedScope`])。
fn open_unsaved_dialog(
    weak: WeakEntity<PreviewWindow>,
    scope: UnsavedScope,
    name: SharedString,
    window: &mut Window,
    cx: &mut App,
) {
    window.open_dialog(cx, move |dialog, _window, _cx| {
        let discard = weak.clone();
        let save = weak.clone();
        let message = match scope {
            UnsavedScope::Tab(_) => {
                format!("「{name}」的改动还没写回远端,关掉标签就找不回来了。")
            }
            UnsavedScope::Window => format!("「{name}」的改动还没写回远端,关掉窗口就找不回来了。"),
        };
        dialog
            .title("还有没写回的改动")
            .w(px(420.))
            .child(div().text_sm().child(message))
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("preview-close-discard")
                            .label("丢弃")
                            .on_click({
                                let discard = discard.clone();
                                move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let Some(preview) = discard.upgrade() else {
                                        return;
                                    };
                                    preview.update(cx, |this, cx| match scope {
                                        UnsavedScope::Tab(ix) => this.close_tab(ix, window, cx),
                                        UnsavedScope::Window => window.remove_window(),
                                    });
                                }
                            }),
                    )
                    .child(
                        Button::new("preview-close-cancel")
                            .label("取消")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("preview-close-save")
                            .label("保存")
                            .primary()
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                let Some(preview) = save.upgrade() else {
                                    return;
                                };
                                preview.update(cx, |this, cx| {
                                    // 写回成功了才真关(见 [`PreviewWindow::finish_save`])。
                                    this.save_then_close(scope, window, cx);
                                });
                            }),
                    ),
            )
            .on_cancel(|_, _, _| true)
    });
}

/// 编辑器的焦点句柄;不是文本(没有编辑器)时 `None`。
///
/// 用处是「切到这个标签时把焦点交给它的编辑器」(见 [`PreviewWindow::focus_active`])。
fn editor_focus(tab: &PreviewTab, cx: &App) -> Option<FocusHandle> {
    match &tab.body {
        Body::Text(text) => Some(text.editor.read(cx).focus_handle(cx)),
        _ => None,
    }
}

/// 现在关这个标签要不要拦一下(见 [`CloseAsk`])。
fn close_ask(tab: &PreviewTab) -> CloseAsk {
    let Body::Text(text) = &tab.body else {
        return CloseAsk::Allow;
    };
    if text.is_saving() {
        CloseAsk::Saving
    } else if text.is_dirty() {
        CloseAsk::Confirm
    } else {
        CloseAsk::Allow
    }
}

/// 读回来的文本建好编辑器之后摆进那个标签(建 [`EditorState`] 要窗口,所以只能在 UI 侧做)。
fn show_text(
    tab: &mut PreviewTab,
    loaded: Result<Loaded, String>,
    window: &mut Window,
    cx: &mut Context<PreviewWindow>,
) {
    let body = match loaded {
        Ok(loaded) => {
            // 语言按**扩展名**认(见 [`Language`]):认得出来就带语法高亮,认不出来当纯文本。
            let language = Language::of(&tab.request.name);
            let encoding = loaded.encoding();
            let editor = cx.new(|cx| {
                EditorState::new(window, cx)
                    .language(language.id())
                    .default_value(loaded.text)
            });
            let subs = vec![
                // 改一笔就记一笔版本号(保存时按版本号判断「有没有没写回的改动」)。
                cx.subscribe(&editor, |this, editor, event: &InputEvent, cx| {
                    this.on_editor_event(editor.clone(), event, cx)
                }),
                // 光标 / 选区 / 滚动都会让编辑器 `cx.notify()`,借它把状态栏那几个数刷新掉。
                cx.observe(&editor, |_, _, cx| cx.notify()),
            ];
            Body::Text(TextEditing {
                editor,
                language,
                encoding,
                // 与 gpui-base 编辑器的 `soft_wrap` 默认值一致(见 [`TextEditing::soft_wrap`])。
                soft_wrap: true,
                revisions: Revisions::default(),
                saving: None,
                outcome: None,
                _subs: subs,
            })
        }
        Err(message) => Body::Failed(message),
    };
    tab.body = body;
}

/// 远端那份被写回过 ⇒ 让文件管理器把**它所在的目录**重读一次(树里的大小跟着更新)。
///
/// 找不到父目录(远端路径里没有 `/`)或文件管理器已经没了就当没这回事 —— 这只是锦上添花。
fn refresh_parent_dir(tab: &PreviewTab, cx: &mut App) {
    let Some((dir, _)) = tab.request.remote_path.rsplit_once('/') else {
        return;
    };
    let Some(root) = tab.request.app_root.upgrade() else {
        return;
    };
    let dir = dir.to_string();
    root.update(cx, |root, cx| root.reload_files_dir(tab.request.session, &dir, cx));
}

/// 工具条上两组动作之间的分隔线。
fn toolbar_separator(cx: &App) -> impl IntoElement {    div()
        .flex_none()
        .w(px(1.))
        .h(px(16.))
        .mx_1()
        .bg(cx.theme().border)
}

/// 工具条上一枚「编辑器动作」按钮:点了把 `action` 派发给**当前聚焦的元素**。
///
/// 焦点这时在编辑器上,所以动作落到编辑器的 `on_action` 处理器(见
/// [`PreviewWindow::render_toolbar`])—— 这也是本窗口能触发这些动作的唯一路子:它们的处理器
/// 方法在 gpui-base 里是 `pub(super)`,跨 crate 直接调不到。
fn action_button(
    id: &'static str,
    icon: IconName,
    tooltip: &'static str,
    action: impl gpui::Action + Clone + 'static,
) -> Button {
    Button::new(id)
        .ghost()
        .xsmall()
        .icon(icon)
        .tooltip(tooltip)
        .on_click(move |_, window, cx| window.dispatch_action(Box::new(action.clone()), cx))
}

/// 弹「跳转行」小框:填一个行号,确认后把光标挪到那一行(见 [`PreviewWindow::open_goto_line`])。
///
/// ⚠️ 输入框的状态在**开框之前**建好(照 [`crate::dialog::folder`] 的 `FolderForm`):
/// `open_dialog` 的构造闭包每帧都会被调一次,在里面 `cx.new` 等于每帧换一个新状态机,敲进去
/// 的字会被抹掉。
fn open_goto_line_dialog(editor: Entity<EditorState>, window: &mut Window, cx: &mut App) {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder("行号"));
    window.open_dialog(cx, move |dialog, _window, cx| {
        // 行号填得不对就别让「跳转」能按(与 [`crate::dialog::folder`] 的「创建」同款)。
        let valid = input.read(cx).value().trim().parse::<usize>().is_ok();
        let entered = input.clone();
        let target = editor.clone();
        dialog
            .title("跳转行")
            .w(px(320.))
            .footer(
                DialogFooter::new()
                    .child(
                        DialogClose::new().child(Button::new("preview-goto-cancel").label("取消")),
                    )
                    .child(
                        DialogAction::new().child(
                            Button::new("preview-goto-confirm")
                                .label("跳转")
                                .primary()
                                .disabled(!valid),
                        ),
                    ),
            )
            .on_ok(move |_, window, cx| {
                let line = entered.read(cx).value();
                goto_line(&target, &line, window, cx);
                true
            })
            .child(Input::new(&input))
    });
}

/// 把光标挪到第 `line` 行(1 起)的行首。
///
/// 行号超出范围就夹到第一行 / 最后一行;填的不是数字就当没这回事(调用方已经先挡了一道,
/// 见 [`open_goto_line_dialog`])。
fn goto_line(editor: &Entity<EditorState>, line: &str, window: &mut Window, cx: &mut App) {
    let Ok(line) = line.trim().parse::<usize>() else {
        return;
    };
    editor.update(cx, |state, cx| {
        let last = state.text().lines_len().max(1);
        // 编辑器的 `Position` 从 0 起,用户填的行号从 1 起。
        let position = Position::new((line.clamp(1, last) - 1) as u32, 0);
        // 折起来的行光标进不去 ⇒ 先把盖住它的那道折展平(见 [`EditorState::unfold_at`])。
        state.unfold_at(position, cx);
        state.set_cursor_position(position, window, cx);
    });
}

/// 读文本:先按大小把大的挡掉,再要求是 UTF-8。
fn read_text(path: &Path) -> Result<Loaded, String> {
    let metadata = std::fs::metadata(path).map_err(|error| format!("本地文件出错:{error}"))?;
    if metadata.len() > MAX_TEXT_BYTES {
        return Err(format!(
            "文件有 {},超过文本编辑的 {} 上限,就不在应用里打开了。",
            format::size(metadata.len()),
            format::size(MAX_TEXT_BYTES)
        ));
    }
    let bytes = std::fs::read(path).map_err(|error| format!("本地文件出错:{error}"))?;
    // ⚠️ BOM 留在文本里(编辑器把它当一个零宽字符)⇒ 存回远端时一模一样。
    let bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
    match String::from_utf8(bytes) {
        Ok(text) => Ok(Loaded { text, bom }),
        Err(_) => Err("这不是 UTF-8 文本(可能是二进制文件)。".to_string()),
    }
}

/// 居中一块「图标 + 标题 + 说明」。
fn centered(
    icon: Icon,
    title: impl Into<SharedString>,
    note: Option<&str>,
    cx: &mut App,
) -> gpui::AnyElement {
    let mut body = v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_3()
        .p_6()
        .child(icon.text_color(cx.theme().muted_foreground))
        .child(
            div()
                .max_w_full()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_center()
                .child(title.into()),
        );
    if let Some(note) = note {
        body = body.child(
            div()
                .max_w(px(420.))
                .text_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(note.to_string()),
        );
    }
    body.into_any_element()
}

/// 工具条上那行「写回」小字:在写 ⇒ 进度;写完了 ⇒ 已写回 / 失败。
///
/// 返回 `(文案, 是不是失败)`;没什么可说的就 `None`(没改过、也没写过)。
fn save_label(text: &TextEditing) -> Option<(String, bool)> {
    if let Some(saving) = &text.saving {
        // 总字节数还不知道(刚开写)时给一句话,别显示一个假的百分比。
        let label = match saving.done.saturating_mul(100).checked_div(saving.total) {
            Some(percent) => format!("写回中 {percent}%"),
            None => "写回中…".to_string(),
        };
        return Some((label, false));
    }
    match &text.outcome {
        // ⚠️ 写完又改过 ⇒「已写回」已经不成立,别挂着误导人(那种情况正是脏标记该亮的时候)。
        Some(Ok(())) if !text.is_dirty() => Some(("已写回远端".to_string(), false)),
        Some(Err(message)) => Some((format!("写回失败:{message}"), true)),
        _ => None,
    }
}

impl AppRoot {
    pub(crate) fn preview_has_pending_changes(
        handle: Option<gpui::WindowHandle<Root>>,
        cx: &mut App,
    ) -> bool {
        handle.is_some_and(|handle| {
            handle
                .update(cx, |root, _, cx| {
                    root.view()
                        .clone()
                        .downcast::<PreviewWindow>()
                        .is_ok_and(|view| {
                            view.read(cx).tabs.iter().any(|tab| close_ask(tab) != CloseAsk::Allow)
                        })
                })
                .unwrap_or(false)
        })
    }

    /// 打开一个文件预览。
    ///
    /// **一个窗口多个标签**(见模块文档):窗口没开就开一个(首标签就是这个请求),已经开了就
    /// 往里加一个标签(同一个远端路径的直接切过去)。主窗口关掉时这个窗口跟着退出应用,见
    /// [`PreviewWindow::new`] 的订阅。
    pub(crate) fn open_preview_window(&mut self, request: PreviewRequest, cx: &mut App) {
        // 窗口已经开着(而且根视图还是我们的 `Root`):直接把请求交给它加标签。
        if let Some(handle) = self.preview_window {
            // ⚠️ `request` 后面还要用来开新窗口(句柄失效那条路),所以这里克隆一份进闭包。
            let add = request.clone();
            let handed_off = handle
                .update(cx, move |root, window, cx| {
                    // `Root` 里包的正是 [`PreviewWindow`](窗口是本模块开的,类型对得上)。
                    let Ok(view) = root.view().clone().downcast::<PreviewWindow>() else {
                        return false;
                    };
                    view.update(cx, |preview, cx| preview.add_request(add, window, cx));
                    // 顺带把窗口抬到前面(用户刚点了「打开」,该看到它)。
                    window.activate_window();
                    true
                })
                .unwrap_or(false);
            if handed_off {
                return;
            }
            // 句柄失效(窗口被系统关了但 `AppRoot` 还没清理)⇒ 当作没开,往下走开新窗口。
            self.preview_window = None;
        }

        let Some(main_window) = cx.active_window() else {
            log::warn!("没有活动窗口,不打开预览");
            return;
        };
        let title = request.name.clone();
        let bounds = Bounds::centered(None, size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)), cx);
        let mut options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(MIN_WINDOW_SIZE.0), px(MIN_WINDOW_SIZE.1))),
            // 独立顶层窗口、不模态(可以对着终端看)。
            kind: WindowKind::Normal,
            ..TitleBar::window_options()
        };
        // 任务栏 / Alt-Tab 里的窗口名(⚠️ 自绘标题栏里不显示它,见模块文档)。
        if let Some(titlebar) = options.titlebar.as_mut() {
            titlebar.title = Some(title);
        }

        match cx.open_window(options, move |window, cx| {
            let view = cx.new(|cx| PreviewWindow::new(request, main_window, window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        }) {
            Ok(handle) => self.preview_window = Some(handle),
            Err(error) => log::error!("failed to open preview window: {error:#}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FileKind;

    #[test]
    fn preview_identity_is_not_reused_after_close() {
        let closed = super::PreviewId::new();
        let reopened = super::PreviewId::new();
        assert_ne!(closed, reopened);
        let active = [reopened];
        assert!(!active.contains(&closed));
    }

    #[test]
    fn preview_identities_are_unique() {
        let mut identities = std::collections::HashSet::new();
        for _ in 0..1024 {
            assert!(identities.insert(super::PreviewId::new().0));
        }
    }

    #[test]
    fn old_save_callback_does_not_match_next_save() {
        let old = super::SaveOperationId::new();
        let current = super::SaveOperationId::new();
        let saving = super::Saving {
            id: current,
            revision: 2,
            done: 0,
            total: 10,
        };
        assert!(!super::Saving::matches(Some(&saving), old));
        assert!(super::Saving::matches(Some(&saving), current));
        assert!(!super::Saving::matches(None, current));
    }

    /// 分类错了会走错界面(拿图片当文本读 / 把音频当成能在应用里看),所以这里逐类钉一遍。
    #[test]
    fn kinds_follow_the_extension() {
        for name in ["a.txt", "README", "Makefile", "notes.md", "data.json", "s.sh"] {
            assert_eq!(FileKind::of(name), FileKind::Text, "{name} 应当是文本");
        }
        // 图片那一类的清单直接用 gpui 图片元素自己的(`Img::extensions`),所以这里只抽查
        // 两个一定在清单里的,以及 `svg` 这种「看着像文本、其实走图片」的。
        for name in ["photo.png", "img.JPEG", "logo.svg", "anim.gif", "pic.webp"] {
            assert_eq!(FileKind::of(name), FileKind::Image, "{name} 应当是图片");
        }
        for name in ["song.mp3", "a.wav", "b.FLAC", "c.ogg", "d.m4a"] {
            assert_eq!(FileKind::of(name), FileKind::Audio, "{name} 应当是音频");
        }
        for name in ["clip.mp4", "movie.MKV", "v.webm", "old.avi"] {
            assert_eq!(FileKind::of(name), FileKind::Video, "{name} 应当是视频");
        }
        for name in ["a.bin", "no-extension", "thing.xyz", "archive.tar.gz"] {
            assert_eq!(FileKind::of(name), FileKind::Other, "{name} 应当是其他");
        }
    }

    /// 大小写不敏感(用户拿到的名字什么样都有)。
    #[test]
    fn kind_ignores_case() {
        assert_eq!(FileKind::of("A.TXT"), FileKind::Text);
        assert_eq!(FileKind::of("B.PNG"), FileKind::Image);
    }

    /// 音频 / 视频交给系统默认程序,其余在应用里看 —— 文件管理器那条分岔就靠它
    /// (见 `FileKind::handed_to_system`)。
    #[test]
    fn only_media_is_handed_to_the_system() {
        for name in ["song.mp3", "a.wav", "b.FLAC", "clip.mp4", "movie.MKV"] {
            assert!(
                FileKind::of(name).handed_to_system(),
                "{name} 应当交给系统默认程序打开"
            );
        }
        for name in ["notes.md", "photo.png", "a.bin", "no-extension", "archive.tar.gz"] {
            assert!(
                !FileKind::of(name).handed_to_system(),
                "{name} 不该走系统程序(应用里有得看)"
            );
        }
    }

    use super::Revisions;

    /// 写回是异步的:开写之后用户还能接着改,那之后回来不能把「已写回」记到最新那一版上,
    /// 否则脏标记会消失、没写回的改动看起来像是存过的。
    #[test]
    fn editing_while_a_write_back_is_in_flight_keeps_the_dirty_mark() {
        let mut revisions = Revisions::default();
        assert!(!revisions.is_dirty(), "刚打开时不该是脏的");

        // 改一笔,抓快照开写。
        revisions.edited();
        let snapshot = revisions.current;
        assert!(revisions.is_dirty());

        // 路上又改了一笔,然后写回成功。
        revisions.edited();
        revisions.mark_saved(snapshot);
        assert!(revisions.is_dirty(), "写回之后又改过的仍应当是脏的");

        // 再抓一次快照写回(这次路上没人改),脏标记才该落。
        revisions.mark_saved(revisions.current);
        assert!(!revisions.is_dirty());
    }

    use super::{Language, caret, counts};
    use gpui_kit::component::highlighter::LanguageRegistry;
    use gpui_kit::component::input::Rope;

    /// 语言认错了会拿错语法染色(`.sh` 当 JSON 染就是这么来的),所以常见扩展名逐个钉一遍。
    #[test]
    fn languages_follow_the_extension() {
        for (name, expected) in [
            ("deploy.sh", Language::Shell),
            ("test_jstate.sh", Language::Shell),
            ("a.BASH", Language::Shell),
            ("tool.py", Language::Python),
            ("stubs.pyi", Language::Python),
            ("config.json", Language::Json),
            ("compose.yaml", Language::Yaml),
            ("Cargo.toml", Language::Toml),
            ("main.rs", Language::Rust),
            ("index.js", Language::JavaScript),
            ("view.jsx", Language::JavaScript),
            ("app.ts", Language::TypeScript),
            ("app.tsx", Language::Tsx),
            ("main.c", Language::C),
            ("vec.hpp", Language::Cpp),
            ("notes.md", Language::Markdown),
            // 认不出来的当纯文本,不硬套一个语法。
            ("notes.txt", Language::Plain),
            ("data.csv", Language::Plain),
            ("archive.tar.gz", Language::Plain),
            ("Makefile", Language::Plain),
            ("main.go", Language::Plain),
        ] {
            assert_eq!(Language::of(name), expected, "{name} 的语言认错了");
        }
    }

    /// ⚠️ 这份枚举与 `Cargo.toml` 的 `tree-sitter-*` feature 是一对:漏开 feature 时语法树
    /// 压根不在注册表里,编辑器只会把内容当普通文本画 —— **不报错**,所以在这里钉死。
    #[test]
    fn every_language_with_a_grammar_is_registered() {
        let registered = LanguageRegistry::singleton().languages();
        for language in [
            Language::Shell,
            Language::Python,
            Language::Json,
            Language::Yaml,
            Language::Toml,
            Language::Rust,
            Language::JavaScript,
            Language::TypeScript,
            Language::Tsx,
            Language::C,
            Language::Cpp,
            Language::Markdown,
        ] {
            let id = language.id();
            assert!(
                registered.iter().any(|name| name.as_ref() == id),
                "{id} 没在 gpui-kit 的语言注册表里(Cargo.toml 少开了 tree-sitter feature?)"
            );
        }
        // 纯文本本来就没有语法,它应当是「注册了但没有语法树」的那一条。
        assert!(registered.iter().any(|name| name.as_ref() == Language::Plain.id()));
    }

    /// 状态栏那三个数都**从 1 起**;列与位置按**字符**算(一个汉字算一个)。
    #[test]
    fn caret_counts_lines_and_characters_from_one() {
        let text = Rope::from("abc\nde\n");
        assert_eq!(caret(&text, 0), (1, 1, 1), "开头");
        assert_eq!(caret(&text, 2), (1, 3, 3), "第一行第三列");
        assert_eq!(caret(&text, 4), (2, 1, 5), "换行之后的第二行开头");
        assert_eq!(caret(&text, 7), (3, 1, 8), "末尾那个空行");
        assert_eq!(counts(&text), (3, 7), "3 行 7 个字符(含换行)");

        let cjk = Rope::from("中\n文\n");
        assert_eq!(caret(&cjk, 3), (1, 2, 2), "汉字占一列");
        assert_eq!(caret(&cjk, 7), (2, 2, 4), "第二行的汉字");
        assert_eq!(counts(&cjk), (3, 4));
    }
}
