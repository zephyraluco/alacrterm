//! Editor construction, editing actions and status presentation.
use super::text_document::*;
use super::{Body, EditorTab, EditorWindow};
use crate::assets::IconName;
use gpui::{App, AppContext as _, Context, Entity, Subscription, Window};
use gpui::{ParentElement as _, px};
use gpui_kit::component::{
    Disableable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    input::{EditorState, Input, InputEvent, InputState, Position, Rope, RopeExt as _},
};

/// 预览编辑器认得的语言。
///
/// ⚠️ 每一条都要在 `Cargo.toml` 里开对应的 `tree-sitter-*` feature(那张清单与这份枚举是
/// 一对):没开的话 gpui-kit 的注册表里没有这个语法,[`Self::id`] 就只是个没有高亮的普通
/// 模式 —— 不会报错,只是不染色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Language {
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
    pub(super) fn of(name: &str) -> Self {
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
    pub(super) fn id(self) -> &'static str {
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
    pub(super) fn label(self) -> &'static str {
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

/// 光标在哪:`(行, 列, 整篇里的字符位置)`,**都从 1 起**(照编辑器的习惯)。
///
/// `列` 按**字符**算(一个汉字占一列),不是字节。
pub(super) fn caret(text: &Rope, offset: usize) -> (usize, usize, usize) {
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
pub(super) fn counts(text: &Rope) -> (usize, usize) {
    (text.lines_len(), text.len_chars())
}

/// 「文本」正文:编辑器 + 写回远端那本账。
pub(super) struct TextEditing {
    pub(super) editor: Entity<EditorState>,
    /// 状态栏上标的语言(见 [`Language`])。
    pub(super) language: Language,
    /// 自动换行开着没有(工具条那枚开关的当前值)。
    ///
    /// 本窗口得自己记着:gpui-base 的编辑器只在构造时收这个开关(`set_soft_wrap`),没有读回来的
    /// 方法。⚠️ 初值必须与编辑器的默认值一致(它的 `soft_wrap` 默认 **true**),否则开关的显示
    /// 状态会和编辑器里的实际行为对不上。
    pub(super) soft_wrap: bool,
    /// 改动 / 写回到哪一版了。
    pub(super) document: TextDocument,
    /// 编辑器的订阅:改动记版本 + 光标动了要刷新状态栏(状态栏那几个数是每帧现读的,
    /// 见 [`EditorWindow::render_status_bar`])。
    pub(super) _subs: Vec<Subscription>,
}

impl TextEditing {
    pub(super) fn is_dirty(&self) -> bool {
        self.document.revisions.is_dirty()
    }

    pub(super) fn is_saving(&self) -> bool {
        self.document.saving.is_some()
    }
}

/// 读回来的文本建好编辑器之后摆进那个标签(建 [`EditorState`] 要窗口,所以只能在 UI 侧做)。
pub(super) fn show_text(
    tab: &mut EditorTab,
    loaded: Result<Loaded, String>,
    window: &mut Window,
    cx: &mut Context<EditorWindow>,
) {
    let body = match loaded {
        Ok(loaded) => {
            // 语言按**扩展名**认(见 [`Language`]):认得出来就带语法高亮,认不出来当纯文本。
            let language = Language::of(&tab.request.name);
            let document = TextDocument::new(loaded.format, loaded.version);
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
            Body::Text(Box::new(TextEditing {
                editor,
                language,
                document,
                // 与 gpui-base 编辑器的 `soft_wrap` 默认值一致(见 [`TextEditing::soft_wrap`])。
                soft_wrap: true,
                _subs: subs,
            }))
        }
        Err(message) => Body::Failed(message),
    };
    tab.body = body;
}

/// 工具条上一枚「编辑器动作」按钮:点了把 `action` 派发给**当前聚焦的元素**。
///
/// 焦点这时在编辑器上,所以动作落到编辑器的 `on_action` 处理器(见
/// [`EditorWindow::render_toolbar`])—— 这也是本窗口能触发这些动作的唯一路子:它们的处理器
/// 方法在 gpui-base 里是 `pub(super)`,跨 crate 直接调不到。
pub(super) fn action_button(
    id: &'static str,
    icon: IconName,
    tooltip: &'static str,
    action: impl gpui::Action + Clone + 'static,
) -> Button {
    Button::new(id)
        .ghost()
        .xsmall()
        .icon(crate::assets::ui_icon(icon))
        .tooltip(tooltip)
        .on_click(move |_, window, cx| window.dispatch_action(Box::new(action.clone()), cx))
}

/// 弹「跳转行」小框:填一个行号,确认后把光标挪到那一行(见 [`EditorWindow::open_goto_line`])。
///
/// ⚠️ 输入框的状态在**开框之前**建好(照 [`crate::dialog::folder`] 的 `FolderForm`):
/// `open_dialog` 的构造闭包每帧都会被调一次,在里面 `cx.new` 等于每帧换一个新状态机,敲进去
/// 的字会被抹掉。
pub(super) fn open_goto_line_dialog(
    editor: Entity<EditorState>,
    window: &mut Window,
    cx: &mut App,
) {
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
pub(super) fn goto_line(
    editor: &Entity<EditorState>,
    line: &str,
    window: &mut Window,
    cx: &mut App,
) {
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

/// 工具条上那行「写回」小字:在写 ⇒ 进度;写完了 ⇒ 已写回 / 失败。
///
/// 返回 `(文案, 是不是失败)`;没什么可说的就 `None`(没改过、也没写过)。
pub(super) fn save_label(text: &TextEditing) -> Option<(String, bool)> {
    if let Some(saving) = &text.document.saving {
        if saving.phase == SavePhase::Checking {
            return Some(("检查远端修改…".into(), false));
        }
        // 总字节数还不知道(刚开写)时给一句话,别显示一个假的百分比。
        let label = match saving.done.saturating_mul(100).checked_div(saving.total) {
            Some(percent) => format!("写回中 {percent}%"),
            None => "写回中…".to_string(),
        };
        return Some((label, false));
    }
    match &text.document.outcome {
        // ⚠️ 写完又改过 ⇒「已写回」已经不成立,别挂着误导人(那种情况正是脏标记该亮的时候)。
        Some(Ok(())) if !text.is_dirty() => Some(("已写回远端".to_string(), false)),
        Some(Err(message)) => Some((format!("写回失败:{message}"), true)),
        _ => None,
    }
}
