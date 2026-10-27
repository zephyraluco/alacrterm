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

pub mod completion;
pub mod completion_service;
mod completion_controller;
mod contrast;
mod prompt_input;
mod scrollbar;
mod suggestion_popup;

mod suggestions;
pub use suggestions::Suggestions as SuggestionModel;
pub use settings::SuggestionSettings;
pub mod terminal_element;

use std::{ops::Range as StdRange, path::PathBuf, sync::Arc, time::Duration};

use gpui::{
    App, AppContext as _, AsyncApp, Entity, FocusHandle, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, ParentElement, Render, ScrollWheelEvent,
    SharedString, StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window, div, rgb,
};
use terminal::{
    Event as TerminalEvent, HostKeyPrompt, Modes, SshFs, Terminal, TerminalBounds, TerminalBuilder,
    TerminalTarget, LaunchOptions,
};

use crate::terminal_element::{TerminalElement, TerminalRenderSettings};

pub use crate::terminal_element::TerminalRenderSettings as RenderSettings;

/// 主机密钥确认的回调：应用层据此弹窗，用户选完调 [`HostKeyPrompt::respond`]。
///
/// 握手线程在等回答（最多 `ssh::HOST_KEY_TIMEOUT`），所以回调必须**尽快**把窗口弹出来，
/// 不能在这里阻塞等待；也不能直接放弃 —— 不回答的话连接要等超时。
pub type HostKeyPromptHandler = Arc<dyn Fn(&HostKeyPrompt, &mut App) + Send + Sync + 'static>;

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
/// 打在终端视图根节点上，让 keymap 里 `"context": "Terminal"` 的绑定只在终端聚焦时生效
/// （默认 keymap 用它压掉 `Root` 的 tab / shift-tab 焦点切换、绑复制粘贴与滚动键，
/// 见 `assets/keymaps/default-*.json`）。
///
/// ⚠️ 绑在这个 context 上的键**不会**再进 PTY：gpui 的按键派发是「动作优先」——
/// 先按 context 深度 + 注册顺序派发 `Action`（命中即默认停止传播），然后才轮到
/// key listener（`TerminalView::on_key_down` → `Terminal::try_keystroke`）。
const TERMINAL_KEY_CONTEXT: &str = "Terminal";

/// 终端视图。
pub struct TerminalView {
    focus_handle: FocusHandle,
    terminal: Option<Entity<Terminal>>,
    subscription: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
    error: Option<String>,
    remote: bool,
    settings: Arc<TerminalRenderSettings>,
    title: SharedString,
    /// IME 组合文本状态（由 `TerminalElement` 在渲染时读取）。
    pub(crate) ime_state: Option<ImeState>,
    /// 光标闪烁相位。
    cursor_phase: bool,
    focused: bool,
    terminal_blinking: Option<bool>,
    completion: completion_controller::CompletionController,
    suggestion_scroll: gpui_kit::component::VirtualListScrollHandle,
    _blink_task: gpui::Task<()>,
    _startup_task: gpui::Task<()>,
    recovery_task: gpui::Task<()>,
    grid_cache: std::cell::RefCell<terminal_element::GridCache>,
    scrollbar_target: std::rc::Rc<std::cell::Cell<Option<usize>>>,
    /// 主机密钥确认的回调（仅远端会话有）。
    host_key_prompt: Option<HostKeyPromptHandler>,
    launch_options: LaunchOptions,
}

impl TerminalView {
    /// 创建终端视图并异步启动终端（本地 = PTY + 事件循环；远端 = russh 会话）。
    ///
    /// `target` 决定连哪儿（见 [`TerminalTarget`]）；`settings` 由应用层提供（渲染参数，
    /// 应用层负责从配置文件解析后传入）；`host_key` 用来问用户「首次连接，信任这台主机吗」
    /// （见 [`HostKeyPromptHandler`]）—— 只有远端会话 + `HostKeyPolicy::Ask` 用得到，
    /// 本地会话传 `None` 即可。
    pub fn new(
        working_directory: Option<PathBuf>,
        target: TerminalTarget,
        host_key: Option<HostKeyPromptHandler>,
        launch_options: LaunchOptions,
        settings: Arc<TerminalRenderSettings>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let remote = target.is_ssh();
        let env = std::env::vars().collect();
        let builder = TerminalBuilder::new_with_options(
            working_directory, target, env, launch_options.clone(), cx,
        );
        let focus_handle = cx.focus_handle();

        // 焦点进出：通知终端应用（FOCUS_IN_OUT 模式），并设置默认光标形状
        let focus_in = cx.on_focus_in(&focus_handle, window, |terminal_view, _window, cx| {
            terminal_view.focus_in(cx);
        });
        let focus_out = cx.on_focus_out(
            &focus_handle,
            window,
            |terminal_view, _event, _window, cx| {
                terminal_view.focus_out(cx);
            },
        );

        // 光标闪烁定时器
        let blink_task = cx.spawn(|this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                loop {
                    cx.background_executor().timer(CURSOR_BLINK_INTERVAL).await;
                    if this
                        .update(&mut cx, |this, cx| {
                            if this.focused
                                && this.settings.cursor_blinks
                                && this.terminal_blinking != Some(false)
                                && !this.has_exited(cx)
                                && !this.terminal.as_ref().is_some_and(|term| {
                                    term.read(cx)
                                        .last_content()
                                        .mode
                                        .contains(Modes::ALT_SCREEN)
                                })
                            {
                                this.cursor_phase = !this.cursor_phase;
                                cx.notify();
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }
        });

        let startup_task = cx.spawn(|this: WeakEntity<Self>, cx: &mut AsyncApp| {
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
                        terminal.update(cx, |terminal, _| {
                            terminal.set_launch_options(this.launch_options.clone());
                        });
                        this.terminal = Some(terminal.clone());
                        this.subscription = Some(subscription);
                        // 终端实体的**通知**也要让本视图重绘：鼠标交互（拖选 / 滚轮 / 点击）
                        // 只排一条 `InternalEvent` + notify（通知的是 `Terminal`），而那条队列
                        // 要等 `TerminalElement::prepaint` 里的 `sync` 才被消费 —— 视图不重绘
                        // 就等于「拖选不跟手」。原因与实测见 `docs/terminal-architecture.md` §4.4。
                        this._subscriptions.push(cx.observe(&terminal, |_, _, cx| cx.notify()));
                        cx.notify();
                    });
                });
            }
        });

        Self {
            focus_handle,
            terminal: None,
            subscription: None,
            _subscriptions: vec![focus_in, focus_out],
            error: None,
            remote,
            settings: settings.clone(),
            title: "终端".into(),
            ime_state: None,
            cursor_phase: true,
            focused: false,
            terminal_blinking: None,
            completion: completion_controller::CompletionController::new(settings.suggestions),
            suggestion_scroll: gpui_kit::component::VirtualListScrollHandle::new(),
            _blink_task: blink_task,
            _startup_task: startup_task,
            recovery_task: gpui::Task::ready(()),
            grid_cache: Default::default(),
            scrollbar_target: Default::default(),
            host_key_prompt: host_key,
            launch_options,
        }
    }

    /// Install the host's completion providers before starting user interaction.
    pub fn set_completion_service(&mut self, service: Arc<dyn completion_service::CompletionService>) {
        self.completion.cancel();
        self.completion.model.service = Some(service);
    }

    /// Retry only a disconnected SSH pane; keep the same view and scrollback.
    pub fn reconnect(&mut self, cx: &mut gpui::Context<Self>) {
        if !self.remote || !self.has_exited(cx) {
            return;
        }
        if let Some(terminal) = &self.terminal {
            if terminal.update(cx, |terminal, cx| terminal.reconnect(cx)) {
                self.recovery_task = gpui::Task::ready(());
                self.completion.reset_session();
                self.terminal_blinking = None;
                self.ime_state = None;
                cx.notify();
            }
        }
    }

    /// Host policy for future reconnects; rendering does not choose startup behavior.
    pub fn set_launch_options(&mut self, options: LaunchOptions, cx: &mut gpui::Context<Self>) {
        self.launch_options = options;
        if let Some(terminal) = &self.terminal {
            terminal.update(cx, |terminal, _| {
                terminal.set_launch_options(self.launch_options.clone());
            });
        }
    }

    /// 运行期替换渲染参数（设置窗口改完立刻生效，不必重启）。
    /// 渲染读的是 `self.settings`（同一个 `Arc` 也传给了 `TerminalElement`）。
    /// 换掉它 + `notify` 即可重绘；光标形状还要同步给终端实体
    /// ——`focus_in` 里做的是同一件事（终端可能被应用改成 DECSCUSR 指定的形状）。
    pub fn set_render_settings(
        &mut self,
        settings: Arc<TerminalRenderSettings>,
        cx: &mut gpui::Context<Self>,
    ) {
        self.settings = settings;
        self.completion.configure(self.settings.suggestions);
        if let Some(terminal) = &self.terminal {
            terminal.update(cx, |terminal, _| {
                terminal.set_cursor_shape(self.settings.cursor_shape);
            });
        }
        cx.notify();
    }

    /// 焦点进入：设置默认光标形状并通知终端应用。
    fn focus_in(&mut self, cx: &mut gpui::Context<Self>) {
        self.focused = true;
        self.cursor_phase = true;
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
        self.focused = false;
        self.completion.dismiss();
        if let Some(terminal) = &self.terminal {
            terminal.update(cx, |terminal, _| terminal.focus_out());
        }
        cx.notify();
    }

    fn handle_terminal_event(&mut self, event: &TerminalEvent, cx: &mut gpui::Context<Self>) {
        match event {
            TerminalEvent::ShellCommand(command) => self.completion.model.add_command(command),
            TerminalEvent::ShellContext(key, value) => self.completion.model.context(key, value),
            TerminalEvent::Prompt(event) => {
                let generation = self
                    .terminal
                    .as_ref()
                    .map_or(0, |terminal| terminal.read(cx).content_generation());
                if self.completion.on_prompt(*event, generation) {
                    self.wake_after_input_recovery(cx);
                }
                cx.notify();
            }
            TerminalEvent::Wakeup | TerminalEvent::SelectionsChanged => cx.notify(),
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
            TerminalEvent::BlinkChanged(blinking) => {
                self.terminal_blinking = Some(*blinking);
                self.cursor_phase = true;
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
                self.completion.invalidate_input();
                cx.notify();
            }
            TerminalEvent::HostKeyPrompt(prompt) => {
                // 首次连接某台主机：交给应用弹窗（握手线程在等回答）。
                if let Some(handler) = &self.host_key_prompt {
                    handler(prompt, cx);
                }
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
        self.completion.dismiss();
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
            if self.terminal.as_ref().is_some_and(|terminal| {
                let terminal = terminal.read(cx);
                terminal.vi_mode_enabled()
                    || terminal.last_content().display_offset != 0
                    || terminal
                        .last_content()
                        .mode
                        .intersects(Modes::ALT_SCREEN | Modes::MOUSE_MODE)
            }) {
                self.completion.invalidate_input();
            }
            self.completion.model.input(text);
            self.schedule_suggestions(false, cx);
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

    /// 对底层终端做一件事并触发重绘。
    ///
    /// ⚠️ 终端把请求排进自己的事件队列（`copy` / `clear` / `Scroll(..)` 都是），
    /// 队列要等 [`TerminalElement::prepaint`](crate::terminal_element::TerminalElement)
    /// 里的 `Terminal::sync` 才被消费 —— 不重绘，事件就躺在队列里不动。
    fn with_terminal(&mut self, cx: &mut gpui::Context<Self>, action: impl FnOnce(&mut Terminal)) {
        let Some(terminal) = &self.terminal else {
            return;
        };
        terminal.update(cx, |terminal, cx| {
            action(terminal);
            cx.notify();
        });
    }

    /// 复制选中内容（`Ctrl+Shift+C`）。
    pub fn copy_text(&mut self, cx: &mut gpui::Context<Self>) {
        self.with_terminal(cx, |terminal| terminal.copy(None));
    }

    /// 粘贴剪贴板内容（`Ctrl+Shift+V`）。
    pub fn paste_text(&mut self, cx: &mut gpui::Context<Self>) {
        self.completion.invalidate_input();
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        self.with_terminal(cx, |terminal| terminal.paste(&text));
    }

    /// 清屏（`Ctrl+Shift+K`）。
    pub fn clear_terminal(&mut self, cx: &mut gpui::Context<Self>) {
        self.with_terminal(cx, |terminal| terminal.clear());
    }

    /// 切换 vi 模式（`Ctrl+Shift+Space`）。
    pub fn toggle_vi_mode(&mut self, cx: &mut gpui::Context<Self>) {
        self.with_terminal(cx, |terminal| terminal.toggle_vi_mode());
    }

    /// 滚动类动作：全屏 TUI（`ALT_SCREEN`）里同一按键属于那个程序，要原样转发给 PTY。
    ///
    /// 动作处理器拿不到原始 keystroke（gpui 只递 `Action`），所以这里自己造转义序列 ——
    /// 与 `terminal::mappings::keys::to_esc_str` 给这几个键的序列一致
    /// （`shift-pageup` = `CSI 5 ~`、`shift-pagedown` = `CSI 6 ~`、`shift-home` / `shift-end`
    /// = `CSI H` / `CSI F`、`shift-up` / `shift-down` = `CSI 1 ; 2 A` / `CSI 1 ; 2 B`）。
    fn scroll_action(
        &mut self,
        cx: &mut gpui::Context<Self>,
        alt_screen_sequence: &'static [u8],
        local: impl FnOnce(&mut Terminal),
    ) {
        self.with_terminal(cx, |terminal| {
            if terminal.last_content().mode.contains(Modes::ALT_SCREEN) {
                terminal.input(alt_screen_sequence);
            } else {
                local(terminal);
            }
        });
    }

    /// 上翻一页（`Shift+PageUp`）。
    pub fn scroll_page_up(&mut self, cx: &mut gpui::Context<Self>) {
        self.scroll_action(cx, b"\x1b[5~", |terminal| terminal.scroll_page_up());
    }

    /// 下翻一页（`Shift+PageDown`）。
    pub fn scroll_page_down(&mut self, cx: &mut gpui::Context<Self>) {
        self.scroll_action(cx, b"\x1b[6~", |terminal| terminal.scroll_page_down());
    }

    /// 滚到顶部（`Shift+Home`）。
    pub fn scroll_to_top(&mut self, cx: &mut gpui::Context<Self>) {
        self.scroll_action(cx, b"\x1b[H", |terminal| terminal.scroll_to_top());
    }

    /// 滚到底部（`Shift+End`）。
    pub fn scroll_to_bottom(&mut self, cx: &mut gpui::Context<Self>) {
        self.scroll_action(cx, b"\x1b[F", |terminal| terminal.scroll_to_bottom());
    }

    /// 上滚一行（`Shift+Up`）。
    pub fn scroll_line_up(&mut self, cx: &mut gpui::Context<Self>) {
        self.scroll_action(cx, b"\x1b[1;2A", |terminal| terminal.scroll_line_up());
    }

    /// 下滚一行（`Shift+Down`）。
    pub fn scroll_line_down(&mut self, cx: &mut gpui::Context<Self>) {
        self.scroll_action(cx, b"\x1b[1;2B", |terminal| terminal.scroll_line_down());
    }

    fn schedule_suggestions(&mut self, manual: bool, cx: &mut gpui::Context<Self>) {
        if !self.completion.begin(manual) {
            return;
        }
        self.completion.task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(if manual {
                    Duration::ZERO
                } else {
                    Duration::from_millis(100)
                })
                .await;
            let Ok(request) = this.update(cx, |this, cx| {
                if !this.focused || this.ime_state.is_some() {
                    return None;
                }
                this.completion.model.refresh(manual);
                this.suggestion_scroll
                    .scroll_to_item(0, gpui::ScrollStrategy::Top);
                cx.notify();
                let backend = if this.remote {
                    completion_service::Backend::Remote(this.remote_fs(cx)?)
                } else {
                    completion_service::Backend::Local
                };
                this.completion.request(manual, backend)
            }) else {
                return;
            };
            let Some(request) = request else {
                return;
            };
            let Some(response) = request.resolve(cx.background_executor().clone()).await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if this.focused && this.ime_state.is_none() && this.completion.apply(response) {
                    cx.notify();
                }
            });
        });
    }

    fn request_input_recovery(&mut self, cx: &mut gpui::Context<Self>) {
        let generation = self
            .terminal
            .as_ref()
            .map_or(0, |terminal| terminal.read(cx).content_generation());
        if self.completion.request_recovery(generation) {
            self.wake_after_input_recovery(cx);
        }
    }
    fn wake_after_input_recovery(&mut self, cx: &mut gpui::Context<Self>) {
        self.recovery_task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(prompt_input::RECOVERY_DELAY)
                .await;
            let _ = this.update(cx, |_, cx| cx.notify());
        });
    }
    fn recover_input(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if !self.completion.recovery_due() || self.ime_state.is_some() {
            return;
        }
        let Some(terminal) = &self.terminal else {
            return;
        };
        terminal.update(cx, |terminal, cx| terminal.sync(window, cx));
        let terminal = terminal.read(cx);
        if terminal.vi_mode_enabled()
            || terminal
                .last_content()
                .mode
                .intersects(Modes::ALT_SCREEN | Modes::MOUSE_MODE)
        {
            return;
        }
        if self
            .completion
            .recover_input(terminal.last_content(), terminal.content_generation())
        {
            self.schedule_suggestions(false, cx);
        }
    }
    fn accept_suggestion(&mut self, cx: &mut gpui::Context<Self>) {
        let content = self
            .terminal
            .as_ref()
            .map(|terminal| terminal.read(cx).last_content());
        match self.completion.accept(content) {
            completion_controller::Acceptance::Refresh => self.schedule_suggestions(false, cx),
            completion_controller::Acceptance::Insert { bytes, retrigger } => {
                if let Some(terminal) = &self.terminal {
                    terminal.update(cx, |terminal, _| terminal.input(bytes.into_bytes()));
                }
                if retrigger {
                    self.schedule_suggestions(true, cx);
                }
            }
            completion_controller::Acceptance::None => {}
        }
        cx.notify();
    }

    fn on_key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.ime_state.is_some() {
            return;
        }
        let key = e.keystroke.key.as_str();
        let control = e.keystroke.modifiers.control;
        let plain = !control && !e.keystroke.modifiers.alt && !e.keystroke.modifiers.platform;
        let allowed = self.ime_state.is_none()
            && self.terminal.as_ref().is_some_and(|terminal| {
                let terminal = terminal.read(cx);
                !terminal.vi_mode_enabled()
                    && terminal.last_content().display_offset == 0
                    && !terminal
                        .last_content()
                        .mode
                        .intersects(Modes::ALT_SCREEN | Modes::MOUSE_MODE)
            });
        if !allowed {
            self.completion.invalidate_input();
        }
        if allowed
            && key == "space"
            && control
            && !e.keystroke.modifiers.alt
            && !e.keystroke.modifiers.platform
        {
            self.completion.cancel();
            self.recover_input(window, cx);
            self.completion.model.trigger();
            self.schedule_suggestions(true, cx);
            cx.notify();
            cx.stop_propagation();
            return;
        }
        let control_passthrough =
            control || e.keystroke.modifiers.alt || e.keystroke.modifiers.platform;
        if control_passthrough {
            self.completion.dismiss();
            cx.notify();
        }
        match self.completion.popup_key(
            key,
            plain && !e.keystroke.modifiers.shift,
            allowed && self.focused,
        ) {
            completion_controller::PopupKey::Navigate(delta) => {
                self.completion.model.step(delta);
                self.suggestion_scroll.scroll_to_item(
                    self.completion.model.selected,
                    gpui::ScrollStrategy::Nearest,
                );
                cx.notify();
                cx.stop_propagation();
                return;
            }
            completion_controller::PopupKey::Accept => {
                self.accept_suggestion(cx);
                cx.stop_propagation();
                return;
            }
            completion_controller::PopupKey::Dismiss => {
                self.completion.dismiss();
                cx.notify();
                cx.stop_propagation();
                return;
            }
            completion_controller::PopupKey::Terminal => {}
        }
        let printable =
            plain && (e.keystroke.key_char.is_some() || key.chars().count() == 1 || key == "space");
        if control && matches!(key, "c" | "d") {
            self.completion.invalidate_input();
        } else if key == "enter" && plain {
            self.completion.submit();
        } else if !printable && !matches!(key, "shift" | "control" | "alt" | "cmd" | "super") {
            let refresh_native = self.completion.edit_key(key, control);
            if refresh_native
                && self.completion.has_input_anchor()
                && allowed
                && !(control && matches!(key, "c" | "d"))
            {
                self.request_input_recovery(cx);
            }
            if !control_passthrough {
                self.schedule_suggestions(false, cx);
            }
        }
        if let Some(terminal) = &self.terminal {
            // ⚠️ 复制粘贴 / 滚动 / vi 模式不在这里：那些键由 keymap 绑成应用动作
            // （`Terminal` context），动作优先于 key listener ⇒ 根本走不到这里。
            // 这个 listener 只负责**没被绑定的键** → 转义序列 → PTY。

            // 可打印字符交给 InputHandler（`TerminalElement::paint` 里的 `window.handle_input`），
            // 这里提前返回，避免同一字符被 `try_keystroke` 与 InputHandler 各处理一次。
            // 与 zed 上游一致；vi 模式下字符要当动作处理，所以不走这条捷径。
            if e.prefer_character_input
                && e.keystroke.key_char.is_some()
                && !terminal.read(cx).vi_mode_enabled()
            {
                return;
            }

            let handled = terminal.update(cx, |terminal, _cx| {
                terminal.try_keystroke(&e.keystroke, false)
            });
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

    /// 终端当前的工作目录。
    ///
    /// 取 PTY 前台进程的 cwd（`PtyProcessInfo` 采样，读的是缓存）；远端会话拿不到远端
    /// 路径，因此返回 `None`（远端目录走 [`Self::remote_fs`]）。
    pub fn working_directory(&self, cx: &App) -> Option<PathBuf> {
        self.terminal
            .as_ref()
            .and_then(|terminal| terminal.read_with(cx, |terminal, _| terminal.working_directory()))
    }

    /// 远端会话的 SFTP 句柄（本地会话为 `None`），供「文件管理器」列远端目录。
    ///
    /// 句柄是懒连接的：拿它不会联网，第一次列目录时才建（见 `ssh::SshFs`）。
    /// ⚠️ 先看 [`Self::is_connected`]：连上之前这份参数还不能用于别的连接
    /// （主机密钥可能还没确认）。
    pub fn remote_fs(&self, cx: &App) -> Option<SshFs> {
        self.terminal
            .as_ref()
            .and_then(|terminal| terminal.read_with(cx, |terminal, _| terminal.remote_fs()))
    }

    /// 会话是否正在运行（本地已退出或远端已断开时为 false）。
    pub fn is_connected(&self, cx: &App) -> bool {
        self.terminal
            .as_ref()
            .is_some_and(|terminal| terminal.read_with(cx, |terminal, _| terminal.is_connected()))
    }

    /// 远端 shell 说出来的当前目录（见 `terminal::directory`）；本地会话或
    /// 没说到过时为 `None`。
    ///
    /// 这是「文件管理器跟着终端走」的数据源。远端没有本地进程可采样，目录只能由远端
    /// shell 自己说（精确的 OSC 7，或 Ubuntu 默认 `PS1` 放在窗口标题里的 `\w`）。
    /// ⚠️ 可能是 `~` / `~/x`（标题那条路缩写了家目录），展开由应用层做。
    pub fn remote_directory(&self, cx: &App) -> Option<String> {
        self.terminal.as_ref().and_then(|terminal| {
            terminal.read_with(cx, |terminal, _| {
                terminal.remote_directory().map(str::to_string)
            })
        })
    }

    /// 会话进程是否已结束（本地 shell 退出 / ssh 连接断开等）。
    pub fn has_exited(&self, cx: &App) -> bool {
        self.terminal.as_ref().is_some_and(|terminal| terminal.read(cx).has_exited())
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

        match self.settings.cursor_blinks && self.terminal_blinking != Some(false) {
            false => true,
            true => self.cursor_phase,
        }
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let requested_line = self.scrollbar_target.take();
        if let Some(terminal) = &self.terminal {
            terminal.update(cx, |terminal, cx| {
                if let Some(line) = requested_line {
                    terminal.scroll_to_history_line(line);
                }
                // Wheel and keyboard scrolling are queued too. Apply them before
                // constructing the handle so its thumb matches this frame's grid.
                terminal.sync(window, cx);
            });
        }
        if let Some(terminal) = &self.terminal {
            self.completion
                .capture_input(terminal.read(cx).last_content());
        }
        self.recover_input(window, cx);
        let focused = self.focus_handle.is_focused(window);
        self.focused = focused;
        let cursor_visible = self.should_show_cursor(focused, cx);

        let mut root = div()
            // 一页里可能同时有好几格终端，id 必须逐台区分
            // （`TerminalElement` 自己不贡献 element id）。
            .id(("terminal-view", cx.entity_id()))
            .size_full()
            .relative()
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

        if let Some(terminal) = &self.terminal {
            if let Some((history, offset)) = terminal
                .read(cx)
                .scrollback_state()
                .filter(|(history, _)| *history > 0)
            {
                let line_height = terminal
                    .read(cx)
                    .last_content()
                    .terminal_bounds
                    .line_height
                    .max(gpui::px(1.));
                let handle = scrollbar::HistoryScrollbar::new(
                    history,
                    offset,
                    line_height,
                    self.scrollbar_target.clone(),
                );
                let geometry = handle.clone();
                root = root.child(
                    div()
                        .absolute()
                        .inset_0()
                        .child(
                            gpui::canvas(
                                move |bounds, _, _| geometry.set_bounds(bounds),
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full(),
                        )
                        .child(
                            gpui_kit::component::scroll::Scrollbar::vertical(&handle)
                                .id("terminal-history-scrollbar"),
                        ),
                );
            }
        }

        if let Some(popup) = self.render_suggestions(cx) {
            root = root.child(popup);
        }

        if self.remote && self.has_exited(cx) {
            root = root.child(
                div().absolute().top_0().right_0().p_2().child(
                    div()
                        .id("reconnect-ssh")
                        .px_3()
                        .py_1()
                        .bg(self.settings.colors.terminal_background)
                        .text_color(self.settings.colors.terminal_foreground)
                        .cursor_pointer()
                        .child("已断开 · 重新连接")
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.reconnect(cx);
                            window.focus(&this.focus_handle, cx);
                        })),
                ),
            );
        }
        root
    }
}
