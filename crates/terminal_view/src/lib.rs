//! 终端视图：负责终端实体的生命周期、输入事件与焦点管理。
//!
//! 实际的网格渲染由 [`terminal_element::TerminalElement`] 完成（从 Zed
//! `terminal_view/src/terminal_element.rs` 移植精简），本 crate 只保留：
//! - 终端创建（`TerminalBuilder`）与事件订阅
//! - 键盘输入（`try_keystroke` 转义序列 / 粘贴）
//! - 鼠标右键粘贴、滚动
//! - 焦点与光标闪烁管理
//! - IME 组合文本状态
//! - 高性能网格渲染（批量文本 / 背景矩形 / 块字符 / 光标 / 高亮）

mod contrast;
pub mod terminal_element;

use std::{
    ops::Range as StdRange,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use gpui::{
    App, AppContext as _, AsyncApp, Entity, FocusHandle, IntoElement, InteractiveElement,
    KeyBinding, KeyDownEvent, MouseButton, MouseDownEvent, NoAction, ParentElement, Render,
    ScrollWheelEvent, SharedString, Styled, Subscription, WeakEntity, Window, div, rgb,
};
use terminal::{Event as TerminalEvent, Modes, Terminal, TerminalBounds, TerminalBuilder};
use util::shell::Shell;

use crate::terminal_element::{TerminalElement, TerminalRenderSettings};

pub use crate::terminal_element::TerminalRenderSettings as RenderSettings;

/// 实现 [`gpui::Focusable`]：让应用层能直接聚焦 / 判断某个会话的焦点状态
/// （焦点策略由应用层掌握，见 `AppRoot::on_background_mouse_down`）。
impl gpui::Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// IME（输入法）组合文本状态。
pub(crate) struct ImeState {
    pub(crate) marked_text: String,
}

/// 光标闪烁间隔。
const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(500);

/// 终端的 key context。
///
/// 用途只有一个：在焦点位于终端时**取消** tab / shift-tab 的键位绑定，把按键交回
/// 焦点所在的组件。gpui-component 的 `Root` 把这两个键绑成了焦点切换动作，而 gpui 的
/// 键位派发在 key listener 之前、且动作默认停止传播 ⇒ 终端一个 Tab 都收不到。
/// 用 [`NoAction`] 在更深的 context 上压掉那条绑定后，按键就顺着 gpui 的正常流程
/// 落到焦点元素（终端）的 key listener，与 Enter / 方向键走同一条路；
/// 焦点不在终端时本 context 不参与匹配，`Root` 的 Tab 焦点切换照旧。
const TERMINAL_KEY_CONTEXT: &str = "Terminal";

/// 终端视图。
pub struct TerminalView {
    focus_handle: FocusHandle,
    terminal: Option<Entity<Terminal>>,
    subscription: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
    error: Option<String>,
    /// 会话进程是否已结束（本地 shell 退出 / ssh 断开等）。
    ///
    /// 由 `CloseTerminal` 事件置位；应用据此在状态栏显示「已断开」，
    /// 但不会因此退出程序（见 `handle_terminal_event`）。
    exited: bool,
    settings: Arc<TerminalRenderSettings>,
    title: SharedString,
    /// IME 组合文本状态（由 `TerminalElement` 在渲染时读取）。
    pub(crate) ime_state: Option<ImeState>,
    /// 光标闪烁相位。
    cursor_phase: bool,
}

impl TerminalView {
    /// 创建终端视图并异步启动终端（PTY + 事件循环）。
    ///
    /// `settings` 由应用层提供（渲染参数，应用层负责从配置文件解析后传入）。
    pub fn new(
        working_directory: Option<PathBuf>,
        shell: Shell,
        settings: Arc<TerminalRenderSettings>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        // 焦点在终端时让 tab / shift-tab 归终端（见 [`TERMINAL_KEY_CONTEXT`]）。
        // 只需注册一次——每个标签页都会走 `new`。
        static UNBIND_TAB_KEYS: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        UNBIND_TAB_KEYS.get_or_init(|| {
            cx.bind_keys([
                KeyBinding::new("tab", NoAction, Some(TERMINAL_KEY_CONTEXT)),
                KeyBinding::new("shift-tab", NoAction, Some(TERMINAL_KEY_CONTEXT)),
            ]);
        });

        // 焦点进出：通知终端应用（FOCUS_IN_OUT 模式），并设置默认光标形状
        let focus_in = cx.on_focus_in(&focus_handle, window, |terminal_view, _window, cx| {
            terminal_view.focus_in(cx);
        });
        let focus_out = cx.on_focus_out(&focus_handle, window, |terminal_view, _event, _window, cx| {
            terminal_view.focus_out(cx);
        });

        // 光标闪烁定时器
        cx.spawn(|this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                loop {
                    cx.background_executor().timer(CURSOR_BLINK_INTERVAL).await;
                    let _ = this.update(&mut cx, |this, cx| {
                        this.cursor_phase = !this.cursor_phase;
                        cx.notify();
                    });
                }
            }
        })
        .detach();

        let env = std::env::vars().collect();
        let builder = TerminalBuilder::new(working_directory, shell, env, cx);

        cx.spawn(|this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                let builder = match builder.await {
                    Ok(builder) => builder,
                    Err(error) => {
                        log::error!("failed to spawn terminal: {error:#}");
                        cx.update(|app| {
                            let _ = this.update(app, |this, cx| {
                                this.error = Some(format!("{error:#}"));
                                cx.notify();
                            });
                        });
                        return;
                    }
                };

                let terminal: Entity<Terminal> = cx.new(|cx| builder.subscribe(cx));
                let subscription = cx.subscribe(&terminal, {
                    let this = this.clone();
                    move |_terminal, event, cx| {
                        let _ = this.update(cx, |this, cx| this.handle_terminal_event(event, cx));
                    }
                });

                cx.update(|app| {
                    let _ = this.update(app, |this, cx| {
                        this.terminal = Some(terminal.clone());
                        this.subscription = Some(subscription);
                        // 终端实体的**通知**也要让本视图重绘：鼠标交互（拖选 / 滚轮 / 点击）
                        // 只排一条 `InternalEvent` + notify（通知的是 `Terminal`），而那条队列
                        // 要等 `TerminalElement::prepaint` 里的 `sync` 才被消费 —— 视图不重绘
                        // 就等于「拖选不跟手」。原因与实测见 `docs/terminal-architecture.md` §4.4。
                        cx.observe(&terminal, |_, _, cx| cx.notify()).detach();
                        cx.notify();
                    });
                });
            }
        })
        .detach();

        Self {
            focus_handle,
            terminal: None,
            subscription: None,
            _subscriptions: vec![focus_in, focus_out],
            error: None,
            exited: false,
            settings,
            title: "终端".into(),
            ime_state: None,
            cursor_phase: true,
        }
    }

    /// 运行期替换渲染参数（设置窗口改完立刻生效，不必重启）。
    ///
    /// 渲染读的是 `self.settings`（同一个 `Arc` 也传给了 `TerminalElement`），
    /// 换掉它 + `notify` 即可重绘；光标形状还要同步给终端实体
    /// ——`focus_in` 里做的是同一件事（终端可能被应用改成 DECSCUSR 指定的形状）。
    pub fn set_render_settings(
        &mut self,
        settings: Arc<TerminalRenderSettings>,
        cx: &mut gpui::Context<Self>,
    ) {
        self.settings = settings;
        if let Some(terminal) = &self.terminal {
            terminal.update(cx, |terminal, _| {
                terminal.set_cursor_shape(self.settings.cursor_shape);
            });
        }
        cx.notify();
    }

    /// 焦点进入：设置默认光标形状并通知终端应用。
    fn focus_in(&mut self, cx: &mut gpui::Context<Self>) {
        if let Some(terminal) = &self.terminal {
            terminal.update(cx, |terminal, _| {
                terminal.set_cursor_shape(self.settings.cursor_shape);
                terminal.focus_in();
            });
        }
        cx.notify();
    }

    /// 焦点离开：通知终端应用（未聚焦时光标由渲染层显示为空心）。
    fn focus_out(&mut self, cx: &mut gpui::Context<Self>) {
        if let Some(terminal) = &self.terminal {
            terminal.update(cx, |terminal, _| terminal.focus_out());
        }
        cx.notify();
    }

    fn handle_terminal_event(
        &mut self,
        event: &TerminalEvent,
        cx: &mut gpui::Context<Self>,
    ) {
        match event {
            TerminalEvent::Wakeup | TerminalEvent::SelectionsChanged => cx.notify(),
            // PowerShell 上报了新的工作目录（见 `terminal::platform`）：重绘即可，
            // 文件管理器下一帧会把根目录换过去。
            #[cfg(windows)]
            TerminalEvent::PwshPathChanged => cx.notify(),
            TerminalEvent::TitleChanged | TerminalEvent::BreadcrumbsChanged => {
                if let Some(terminal) = &self.terminal {
                    terminal.read_with(cx, |terminal, _| {
                        let title = if terminal.breadcrumb_text.is_empty() {
                            "终端".to_string()
                        } else {
                            terminal.breadcrumb_text.clone()
                        };
                        self.title = title.into();
                    });
                }
                cx.notify();
            }
            TerminalEvent::CloseTerminal => {
                // 会话进程结束（本地 shell 退出、ssh 连接断开等）。
                //
                // **不要退出应用**：这是多标签终端，一个会话结束不应带走整个程序
                // （尤其在「连接断开」这种常见场景下，直接退出等同于闪退）。
                // 这里只标记状态并刷新界面，由应用决定怎么展示（保留标签、状态栏显示
                // 已断开，用户自行关闭或新建）。终端网格会保留最后的输出，
                // 这样 ssh 报的断开原因不会被丢掉。
                self.exited = true;
                cx.notify();
            }
            _ => {}
        }
    }

    /// 当前终端网格的实际像素尺寸（供 IME 候选窗定位）。
    pub(crate) fn terminal_bounds(&self, cx: &App) -> TerminalBounds {
        match &self.terminal {
            Some(terminal) => {
                terminal.read_with(cx, |terminal, _| terminal.last_content().terminal_bounds)
            }
            None => TerminalBounds::default(),
        }
    }

    /// 设置 IME 组合（pre-edit）文本。
    pub(crate) fn set_marked_text(&mut self, text: String, cx: &mut gpui::Context<Self>) {
        if text.is_empty() {
            return self.clear_marked_text(cx);
        }
        self.ime_state = Some(ImeState { marked_text: text });
        cx.notify();
    }

    /// 获取当前 IME 组合文本范围（UTF-16）。
    pub(crate) fn marked_text_range(&self) -> Option<StdRange<usize>> {
        self.ime_state
            .as_ref()
            .map(|state| 0..state.marked_text.encode_utf16().count())
    }

    /// 清除 IME 组合文本状态。
    pub(crate) fn clear_marked_text(&mut self, cx: &mut gpui::Context<Self>) {
        if self.ime_state.is_some() {
            self.ime_state = None;
            cx.notify();
        }
    }

    /// 把输入法/普通字符输入写入 PTY。
    pub(crate) fn commit_text(&mut self, text: &str, cx: &mut gpui::Context<Self>) {
        if !text.is_empty() {
            if let Some(terminal) = &self.terminal {
                terminal.update(cx, |term, _| {
                    term.input(text.to_string().into_bytes());
                });
            }
        }
    }

    /// 滚轮滚动（由 `TerminalElement` 的滚动监听回调调用）。
    pub(crate) fn scroll_wheel(&mut self, event: &ScrollWheelEvent, cx: &mut gpui::Context<Self>) {
        if let Some(terminal) = &self.terminal {
            terminal.update(cx, |term, _cx| term.scroll_wheel(event, 1.0));
        }
    }

    fn on_key_down(&mut self, e: &KeyDownEvent, _window: &mut Window, cx: &mut gpui::Context<Self>) {
        if let Some(terminal) = &self.terminal {
            // 粘贴：Ctrl+Shift+V
            if e.keystroke.modifiers.control
                && e.keystroke.modifiers.shift
                && e.keystroke.key.eq_ignore_ascii_case("v")
            {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    terminal.update(cx, |terminal, _cx| terminal.paste(&text));
                }
                cx.stop_propagation();
                return;
            }

            // 可打印字符交给 InputHandler（`TerminalElement::paint` 里的 `window.handle_input`），
            // 这里提前返回，避免同一字符被 `try_keystroke` 与 InputHandler 各处理一次。
            // 与 zed 上游一致；vi 模式下字符要当动作处理，所以不走这条捷径。
            if e.prefer_character_input
                && e.keystroke.key_char.is_some()
                && !terminal.read(cx).vi_mode_enabled()
            {
                return;
            }

            let handled =
                terminal.update(cx, |terminal, _cx| terminal.try_keystroke(&e.keystroke, false));
            if handled {
                cx.stop_propagation();
            }
        }
    }

    /// 该终端当前是否持有焦点（供应用层判断点击是否落在终端之外）。
    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus_handle.is_focused(window)
    }

    /// 左键点击终端区域：聚焦，并**停止冒泡**。
    ///
    /// 停止冒泡是为了让上层（`AppRoot`）能区分「点在终端里」与「点在终端之外」。
    fn on_left_mouse_down(
        &mut self,
        _: &MouseDownEvent,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        cx.stop_propagation();
    }

    fn on_mouse_down(
        &mut self,
        e: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        // 右键粘贴。鼠标模式下按住 shift 的右键由 TerminalElement 上报给应用。
        let should_paste = e.button == MouseButton::Right
            && self
                .terminal
                .as_ref()
                .map_or(true, |t| !t.read(cx).mouse_mode(e.modifiers.shift));
        if should_paste {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                if let Some(terminal) = &self.terminal {
                    terminal.update(cx, |terminal, _cx| terminal.paste(&text));
                }
            }
        }
    }

    /// 当前窗口标题（终端标题或默认文本）。
    pub fn title(&self) -> SharedString {
        self.title.clone()
    }

    /// 当前终端进程（PTY 里的 shell / ssh 等）的 PID。
    ///
    /// 供状态栏采样该会话的 CPU / 内存使用；PTY 尚未就绪或终端已退出时为 `None`。
    /// 这里刻意返回裸 `u32` 而不是 `sysinfo::Pid`，避免让本 crate 依赖 sysinfo。
    pub fn pid(&self, cx: &App) -> Option<u32> {
        self.terminal
            .as_ref()
            .and_then(|terminal| terminal.read_with(cx, |terminal, _| terminal.pid()))
            .map(|pid| pid.as_u32())
    }

    /// 终端当前的工作目录（供「文件管理器」侧边栏确定根目录）。
    ///
    /// 本地会话取 shell 自己上报的位置（Windows 的 PowerShell，见 `terminal::platform`）；
    /// 拿不到时回落 PTY 前台进程的 cwd（`PtyProcessInfo` 采样，读的是缓存）。
    /// 远端会话两者都没有，因此返回 `None`。目录变化会以事件上报，所以调用方每次渲染读一下即可。
    pub fn working_directory(&self, cx: &App) -> Option<PathBuf> {
        self.terminal
            .as_ref()
            .and_then(|terminal| terminal.read_with(cx, |terminal, _| terminal.working_directory()))
    }

    /// 会话进程是否已结束（本地 shell 退出 / ssh 连接断开等）。
    pub fn has_exited(&self) -> bool {
        self.exited
    }

    /// 计算光标是否可见（闪烁控制）。
    fn should_show_cursor(&self, focused: bool, cx: &mut gpui::Context<Self>) -> bool {
        // 未聚焦：显示空心光标
        if !focused {
            return true;
        }

        // ALT_SCREEN（vim 等全屏 TUI）：始终显示
        if self
            .terminal
            .as_ref()
            .is_some_and(|t| t.read(cx).last_content().mode.contains(Modes::ALT_SCREEN))
        {
            return true;
        }

        match self.settings.cursor_blinks {
            false => true,
            true => self.cursor_phase,
        }
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        window.set_window_title(&self.title);

        let focused = self.focus_handle.is_focused(window);
        let cursor_visible = self.should_show_cursor(focused, cx);

        let mut root = div()
            .id("terminal-view")
            .size_full()
            .key_context(TERMINAL_KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_left_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .bg(self.settings.colors.terminal_background);

        if let Some(terminal) = &self.terminal {
            root = root.child(TerminalElement::new(
                terminal.clone(),
                cx.entity(),
                self.focus_handle.clone(),
                focused,
                cursor_visible,
                self.settings.clone(),
            ));
        } else if let Some(error) = &self.error {
            root = root.child(
                div()
                    .text_color(rgb(0xff0000))
                    .child(error.clone())
                    .into_element(),
            );
        } else {
            root = root.child(
                div()
                    .text_color(self.settings.colors.terminal_foreground)
                    .child("正在启动终端…"),
            );
        }

        root
    }
}
