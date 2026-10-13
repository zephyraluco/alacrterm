//! 「打开文件」的预览窗口:一个文件一个窗口(照 [`crate::settings_window`] 的模式 ——
//! 独立顶层窗口、自绘标题栏、外层包 gpui-kit `Root`)。
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
//! **文本是一个简易编辑器**:行号 + 语法高亮(语言按扩展名认,见 [`Language`])+ 可改。
//! Ctrl+S(或工具条那枚「保存」)把编辑器里那份快照写回**远端**(见 [`PreviewWindow::save`]);
//! 还没写回就关窗口会先问一句(见 [`PreviewWindow::should_close`])。图片只看不动。
//!
//! ## 什么在应用内、什么交给系统
//!
//! | 类型 | 界面 |
//! |---|---|
//! | 文本 | **简易编辑器**(行号 + 语法高亮 + 可改):Ctrl+S 写回远端;超过 [`MAX_TEXT_BYTES`] 或不是 UTF-8 就不加载 |
//! | 图片 | `img` 元素等比缩小放进窗口(`ObjectFit::ScaleDown`,小图不放大) |
//! | 其他 | 占位说明 + 「用系统程序打开」(没有内嵌预览的类型) |
//!
//! 窗口底部是状态栏(远端路径 + 语言 / 编码 / 光标位置 / 行数字符数,见
//! [`PreviewWindow::render_status_bar`]),正文用 gpui-kit 的 `Editor`。
//!
//! ⚠️ **音频 / 视频不在这里** —— 进程里没有解码器(内嵌播放要么用 Windows Media Foundation
//! 自己写一层 unsafe 的 COM,要么引来 ffmpeg),所以由文件管理器下完缓存**直接交给系统默认
//! 程序**(见 [`crate::sidebar_panel::files`] 的 `open_file`);这个窗口收不到这两种类型。

use std::path::{Path, PathBuf};

use futures::StreamExt as _;
use gpui::{
    AnyWindowHandle, App, AppContext as _, Bounds, ClipboardItem, Context, Entity, Focusable as _,
    InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _, Render, SharedString,
    Styled as _, StyledImage as _, Subscription, WeakEntity, Window, WindowBounds, WindowKind,
    WindowOptions, div, img, px, size,
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Root, Sizable as _, TitleBar, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::DialogFooter,
    h_flex,
    input::{Editor, EditorState, InputEvent, Rope, RopeExt as _},
    notification::Notification,
    status_bar::StatusBar,
    v_flex,
};
use terminal::{SshFs, TransferEvent};

use crate::AppRoot;
use crate::actions::SavePreviewFile;
use crate::assets::IconName;
use crate::format;
use crate::keymap::PREVIEW_CONTEXT;
use crate::status_bar::STATUS_BAR_HEIGHT;
/// 窗口初始尺寸(宽, 高)。
const WINDOW_SIZE: (f32, f32) = (760., 560.);
/// 窗口最小尺寸(宽, 高)。
const MIN_WINDOW_SIZE: (f32, f32) = (420., 300.);
/// 顶部工具条高度。
const TOOLBAR_HEIGHT: gpui::Pixels = px(36.);
/// 文本预览最多加载多少字节。
///
/// 编辑器的语法解析 / 布局是**全量**的,几十 MB 的日志挂上去就动不了了;超了就不加载,
/// 改成「用系统默认程序打开」的提示(见 [`Body::Failed`])。
const MAX_TEXT_BYTES: u64 = 8 * 1024 * 1024;

/// 预览窗口要显示的东西。
#[derive(Clone)]
pub(crate) struct PreviewRequest {
    /// 显示名(远端文件名)。
    pub(crate) name: SharedString,
    /// 远端路径(「复制远端路径」用;文本保存时写回的就是它)。
    pub(crate) remote_path: String,
    /// 已经下到本地的缓存文件。
    pub(crate) local_path: PathBuf,
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
    /// 不内嵌,交给系统(音频 / 视频不会走到这里,见模块文档;这条留给「其他」)。
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
    /// 抓那份文本时的版本号(见 [`Revisions`])。
    revision: u64,
    /// 已经写过去多少字节。
    done: u64,
    /// 这次要写多少字节(工具条上那行进度小字的分母)。
    total: u64,
}

/// 关窗口时该怎么办(见 [`PreviewWindow::should_close`])。
enum CloseAsk {
    /// 放行。
    Allow,
    /// 正在写回:挡下来。
    Saving,
    /// 有没写回的改动:弹确认框。
    Confirm,
}

/// 预览窗口。
pub(crate) struct PreviewWindow {
    request: PreviewRequest,
    kind: FileKind,
    /// 主窗口;它关了就退出应用(照 [`crate::settings_window::SettingsWindow`])。
    _main_window_closed: Subscription,
    body: Body,
    /// 「保存并关闭」:关窗口那个确认框里点了「保存」时为真,写回成功了窗口才真关。
    close_after_save: bool,
}

impl PreviewWindow {
    fn new(
        request: PreviewRequest,
        main_window: AnyWindowHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let kind = FileKind::of(&request.name);

        let main_window_id = main_window.window_id();
        let _main_window_closed = cx.on_window_closed(move |cx, closed| {
            if closed == main_window_id {
                cx.quit();
            }
        });

        let mut this = Self {
            request,
            kind,
            _main_window_closed,
            body: Body::Loading,
            close_after_save: false,
        };
        this.body = match kind {
            FileKind::Text => Body::Loading,
            FileKind::Image => Body::Image,
            // 音频 / 视频由文件管理器直接交给系统默认程序,到不了这个窗口(见 [`FileKind::handed_to_system`])。
            FileKind::Audio | FileKind::Video | FileKind::Other => {
                Body::External("这个类型没有内嵌预览。")
            }
        };
        // 关窗口前拦一道:有没写回的改动时弹确认框(见 [`Self::should_close`])。
        let guard = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| Self::should_close(&guard, window, cx));
        if kind == FileKind::Text {
            this.load_text(window, cx);
        }
        this
    }

    /// 关窗口该不该放行(`on_window_should_close` 的处理器,见 [`Self::new`])。
    ///
    /// 没改动 ⇒ 放行;正在写回 ⇒ 挡一下(写了一半就断,远端那份可能是残的);有没写回的改动
    /// ⇒ 弹确认框(保存 / 丢弃 / 取消)并挡下这一次 —— 用户选了「丢弃」或写回成功后再由那边
    /// 自己把窗口收掉。
    fn should_close(weak: &WeakEntity<Self>, window: &mut Window, cx: &mut App) -> bool {
        // 视图已经没了(窗口在收尾)⇒ 没什么可拦的。
        let Ok((ask, name)) =
            weak.read_with(cx, |this, _| (this.close_ask(), this.request.name.clone()))
        else {
            return true;
        };
        match ask {
            CloseAsk::Allow => true,
            CloseAsk::Saving => {
                window.push_notification(Notification::warning("正在写回远端,写完才能关窗口"), cx);
                false
            }
            CloseAsk::Confirm => {
                // 点两次 X 不该叠两个框。
                if !window.has_active_dialog(cx) {
                    open_unsaved_dialog(weak.clone(), name, window, cx);
                }
                false
            }
        }
    }

    /// 现在关窗口要不要拦一下(见 [`CloseAsk`])。
    fn close_ask(&self) -> CloseAsk {
        let Body::Text(text) = &self.body else {
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

    /// 读本地缓存那份文本,读回来再建编辑器(建 [`EditorState`] 要窗口,所以只能在 UI 侧做)。
    fn load_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.request.local_path.clone();
        cx.spawn_in(
            window,
            async move |this: WeakEntity<Self>, cx: &mut gpui::AsyncWindowContext| {
                let loaded = cx
                    .background_executor()
                    .spawn(async move { read_text(&path) })
                    .await;
                let _ = this.update_in(cx, |this, window, cx| this.show_text(loaded, window, cx));
            },
        )
        .detach();
    }    fn show_text(
        &mut self,
        loaded: Result<Loaded, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let body = match loaded {
            Ok(loaded) => {
                // 语言按**扩展名**认(见 [`Language`]):认得出来就带语法高亮,认不出来当纯文本。
                let language = Language::of(&self.request.name);
                let encoding = loaded.encoding();
                let editor = cx.new(|cx| {
                    EditorState::new(window, cx)
                        .language(language.id())
                        .default_value(loaded.text)
                });
                let subs = vec![
                    // 改一笔就记一笔版本号(保存时按版本号判断「有没有没写回的改动」)。
                    cx.subscribe(&editor, |this, _, event: &InputEvent, cx| {
                        this.on_editor_event(event, cx)
                    }),
                    // 光标 / 选区 / 滚动都会让编辑器 `cx.notify()`,借它把状态栏那几个数刷新掉。
                    cx.observe(&editor, |_, _, cx| cx.notify()),
                ];
                // 打开就能直接敲,不用先去点一下正文。
                let focus = editor.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
                Body::Text(TextEditing {
                    editor,
                    language,
                    encoding,
                    revisions: Revisions::default(),
                    saving: None,
                    outcome: None,
                    _subs: subs,
                })
            }
            Err(message) => Body::Failed(message),
        };
        self.body = body;
        cx.notify();
    }

    /// 编辑器改动:记一笔版本号,并把上一次「已写回 / 失败」的小字清掉(它已经过期了)。
    fn on_editor_event(&mut self, event: &InputEvent, cx: &mut Context<Self>) {
        if !matches!(event, InputEvent::Change) {
            return;
        }
        let Body::Text(text) = &mut self.body else {
            return;
        };
        text.revisions.edited();
        text.outcome = None;
        cx.notify();
    }

    /// Ctrl+S / 工具条「保存」:把编辑器里那份**快照**写回远端。
    ///
    /// 一条链:抓快照 → 落本地缓存那份(编辑器只活在内存里)→ 交给传输 worker 写远端
    /// (见 [`terminal::SshFs::upload_one`])。路上用户接着改不影响这次写回的内容,写完了也只
    /// 把「已写回」记在**快照那一版**上(见 [`Revisions`])。
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Body::Text(text) = &mut self.body else {
            return;
        };
        // 没改过就别写(顺带挡掉「打开就按 Ctrl+S」这种空写);上一次还在路上也别叠第二次。
        if !text.is_dirty() || text.is_saving() {
            return;
        }
        let contents = text.editor.read(cx).value().to_string();
        let total = contents.len() as u64;
        text.saving = Some(Saving {
            revision: text.revisions.current,
            done: 0,
            total,
        });
        text.outcome = None;
        cx.notify();

        let cache = self.request.local_path.clone();
        let remote = self.request.remote_path.clone();
        let fs = self.request.fs.clone();
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
                        this.finish_save(Err(message), window, cx)
                    });
                    return;
                }
                let mut events = fs.upload_one(cache, remote).events;
                while let Some(event) = events.next().await {
                    match event {
                        TransferEvent::Progress(progress) => {
                            let (done, total) = (progress.bytes_done, progress.bytes_total);
                            // 视图已经没了也把这次写回读完:半截的远端文件比多等一会儿糟。
                            let _ = this.update_in(cx, |this, window, cx| {
                                this.on_save_progress(done, total, window, cx)
                            });
                        }
                        TransferEvent::Done(result) => {
                            let result = result.map_err(|error| error.to_string());
                            let _ = this.update_in(cx, |this, window, cx| {
                                this.finish_save(result, window, cx)
                            });
                            return;
                        }
                    }
                }
                // 通道在 `Done` 之前就断了:worker 线程没了(照 `sidebar_panel/files.rs` 那条路)。
                let _ = this.update_in(cx, |this, window, cx| {
                    this.finish_save(Err("远端文件系统线程已退出".to_string()), window, cx)
                });
            },
        )
        .detach();
    }

    /// 写回进度(工具条那行小字)。
    fn on_save_progress(
        &mut self,
        done: u64,
        total: u64,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Body::Text(text) = &mut self.body else {
            return;
        };
        let Some(saving) = text.saving.as_mut() else {
            return;
        };
        saving.done = done;
        if total > 0 {
            saving.total = total;
        }
        cx.notify();
    }

    /// 一次写回收尾(成功 / 失败)。`result` 里带的是已经翻成人话的失败原因。
    fn finish_save(&mut self, result: Result<(), String>, window: &mut Window, cx: &mut Context<Self>) {
        let outcome = {
            let Body::Text(text) = &mut self.body else {
                return;
            };
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
                // 远端那份变了:工具条上的大小跟着变,文件管理器那边把目录重读一次。
                self.request.size = size;
                self.refresh_parent_dir(cx);
                if self.close_after_save {
                    // 关窗口那一下点的「保存」:写回成功了才真的关。
                    window.remove_window();
                }
            }
            Err(message) => {
                // 这次没成 ⇒ 别再自动关窗口,让用户看着错误自己决定。
                self.close_after_save = false;
                let note = format!("写回远端失败:{message}");
                log::warn!("{}:{note}", self.request.remote_path);
                window.push_notification(Notification::error(note), cx);
            }
        }
        cx.notify();
    }

    /// 远端那份被写回过 ⇒ 让文件管理器把**它所在的目录**重读一次(树里的大小跟着更新)。
    ///
    /// 找不到父目录(远端路径里没有 `/`)或文件管理器已经没了就当没这回事 —— 这只是锦上添花。
    fn refresh_parent_dir(&self, cx: &mut Context<Self>) {
        let Some((dir, _)) = self.request.remote_path.rsplit_once('/') else {
            return;
        };
        let Some(root) = self.request.app_root.upgrade() else {
            return;
        };
        let dir = dir.to_string();
        root.update(cx, |root, cx| root.reload_files_dir(&dir, cx));
    }

    /// 交给系统默认程序打开本地缓存那份。
    fn open_externally(&self, cx: &App) {
        cx.open_with_system(&self.request.local_path);
    }

    fn copy_remote_path(&self, cx: &App) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.request.remote_path.clone()));
    }

    /// 顶部工具条:名称 + 大小 + 类型,右边几枚动作(文本那类多一枚「保存」)。
    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 类型 / 语言挪到底部状态栏去了(见 [`Self::render_status_bar`]),这里只留大小。
        let size = self.request.size;
        let text = match &self.body {
            Body::Text(text) => Some(text),
            _ => None,
        };

        let mut title = h_flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap_1()
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_sm()
                    .child(self.request.name.clone()),
            );
        if text.is_some_and(TextEditing::is_dirty) {
            // 有没写回的改动:名字后面点一颗。
            title = title.child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(cx.theme().primary)
                    .child("●"),
            );
        }

        let mut bar = h_flex()
            .h(TOOLBAR_HEIGHT)
            .w_full()
            .flex_none()
            .px_3()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(title);
        if size > 0 {
            bar = bar.child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format::size(size)),
            );
        }

        if let Some(text) = text {
            if let Some((label, failed)) = save_label(text) {
                bar = bar.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(if failed {
                            cx.theme().danger
                        } else {
                            cx.theme().muted_foreground
                        })
                        .child(label),
                );
            }
            bar = bar.child(
                Button::new("preview-save")
                    .outline()
                    .xsmall()
                    .label("保存")
                    .tooltip("写回远端 (Ctrl+S)")
                    .disabled(text.is_saving() || !text.is_dirty())
                    .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
            );
        }

        bar.child(
            Button::new("preview-copy-path")
                .ghost()
                .xsmall()
                .icon(IconName::Copy)
                .tooltip("复制远端路径")
                .on_click(cx.listener(|this, _, _, cx| this.copy_remote_path(cx))),
        )
        .child(
            Button::new("preview-open-external")
                .outline()
                .xsmall()
                .label("用系统程序打开")
                .on_click(cx.listener(|this, _, _, cx| this.open_externally(cx))),
        )
    }

    fn render_body(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        match &self.body {
            Body::Loading => centered(
                Icon::new(self.kind.icon()).size_8(),
                "正在读取…",
                None,
                cx,
            ),
            Body::Image => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .p_3()
                .child(
                    // `ScaleDown`:等比缩到装得下,小图不放大(预览不该把 16×16 拉成满屏)。
                    img(self.request.local_path.clone())
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
                Icon::new(self.kind.icon()).size_8(),
                self.request.name.as_ref(),
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

    /// 底部状态栏:左边是远端路径,右边是语言 / 编码 / 光标位置 / 行数字符数(照 VS Code 的摆法)。
    ///
    /// 用 gpui-kit 的 [`StatusBar`] 画 —— 主窗口那条状态栏也是它(底色 / 上边框 / 字号都由组件
    /// 给),两块看着才一样。⚠️ 右边那几个数是**每帧现读**编辑器状态的:光标一动编辑器就
    /// `cx.notify()`(订阅见 [`Self::show_text`]),所以它们跟着光标走。
    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut bar = StatusBar::new().h(STATUS_BAR_HEIGHT).w_full().flex_none().child(
            div()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(self.request.remote_path.clone()),
        );

        match &self.body {
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
            // 没有编辑器的:至少把类型标出来(大小在工具条上)。
            _ => {
                bar = bar.right(self.kind.label());
            }
        }
        bar
    }
}

impl Render for PreviewWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            // Ctrl+S 绑在 keymap 的 `"Preview"` context 上(见 `crate::keymap` 与
            // `assets/keymaps/default-*.json`)。⚠️ 这个动作**只由本窗口自己接**:它是窗口级的
            // (一个文件一个窗口),不能像别的动作那样丢给 `AppRoot` 的全局监听器 ——
            // 全局那侧拿不到「是哪个预览窗口」。
            .key_context(PREVIEW_CONTEXT)
            .on_action(cx.listener(|this, _: &SavePreviewFile, window, cx| this.save(window, cx)))
            // 顶部:自绘标题栏。
            .child(
                TitleBar::new().child(
                    h_flex()
                        .w_full()
                        .px(px(8.))
                        .gap(px(8.))
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .text_size(px(13.))
                                .text_color(cx.theme().secondary_foreground)
                                .child(self.request.name.clone()),
                        ),
                ),
            )
            .child(self.render_toolbar(cx))
            .child(div().flex_1().min_h_0().overflow_hidden().child(self.render_body(cx)))
            .child(self.render_status_bar(cx))
            // 覆盖层由 gpui-kit `Root` 渲染。
    }
}

impl Drop for PreviewWindow {
    fn drop(&mut self) {
        // 缓存那份是「打开」时临时下的,窗口没了就删(失败也只是留个临时文件)。
        // ⚠️ 有没写回的改动时关窗口那一下先被 [`PreviewWindow::should_close`] 拦下来问了,
        // 所以走到这里的内容要么已经写回远端、要么是用户明确选了「丢弃」。
        if let Err(error) = std::fs::remove_file(&self.request.local_path) {
            log::debug!("清理预览缓存失败:{}:{error}", self.request.local_path.display());
        }
    }
}

/// 弹「还有没写回的改动」确认框:保存 / 丢弃 / 取消(关窗口那一道,见 [`PreviewWindow::should_close`])。
///
/// 三个按钮各自收尾,不再走 `Dialog` 的 ok / cancel 那一对回调 —— 那对只有两个通道,分不出
/// 三个选择。`escape` / 点遮罩 = 取消(什么都不做,只是把框收掉)。
fn open_unsaved_dialog(
    weak: WeakEntity<PreviewWindow>,
    name: SharedString,
    window: &mut Window,
    cx: &mut App,
) {
    window.open_dialog(cx, move |dialog, _window, _cx| {
        let save = weak.clone();
        dialog
            .title("还有没写回的改动")
            .w(px(420.))
            .child(
                div()
                    .text_sm()
                    .child(format!("「{name}」的改动还没写回远端,关掉窗口就找不回来了。")),
            )
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("preview-close-discard")
                            .label("丢弃")
                            .on_click(|_, window, cx| {
                                window.close_dialog(cx);
                                window.remove_window();
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
                                    this.close_after_save = true;
                                    this.save(window, cx);
                                });
                            }),
                    ),
            )
            .on_cancel(|_, _, _| true)
    });
}

/// 读文本:先按大小把大的挡掉,再要求是 UTF-8。
fn read_text(path: &Path) -> Result<Loaded, String> {
    let metadata = std::fs::metadata(path).map_err(|error| format!("本地文件出错:{error}"))?;
    if metadata.len() > MAX_TEXT_BYTES {
        return Err(format!(
            "文件有 {},超过文本预览的 {} 上限,用系统默认程序打开吧。",
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
    /// 打开一个预览窗口。
    ///
    /// 一个文件一个窗口(不做复用:「同一个远端路径」不代表内容没变,复用会让用户以为看的是
    /// 刚下的那份)。主窗口关掉时这些窗口跟着退出应用,见 [`PreviewWindow::new`] 的订阅。
    pub(crate) fn open_preview_window(&mut self, request: PreviewRequest, cx: &mut App) {
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
        // 任务栏 / Alt-Tab 里的窗口名。
        if let Some(titlebar) = options.titlebar.as_mut() {
            titlebar.title = Some(title);
        }

        let opened = cx.open_window(options, move |window, cx| {
            let view = cx.new(|cx| PreviewWindow::new(request, main_window, window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        });
        if let Err(error) = opened {
            log::error!("failed to open preview window: {error:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FileKind;

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
