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
    KeyDownEvent, MouseButton, MouseDownEvent, ParentElement, Render, ScrollWheelEvent,
    SharedString, Styled, Subscription, WeakEntity, Window, div, rgb,
};
use terminal::{Event as TerminalEvent, Modes, Terminal, TerminalBounds, TerminalBuilder};

use crate::terminal_element::{TerminalElement, TerminalRenderSettings};
use util::shell::Shell;

/// IME（输入法）组合文本状态。
pub(crate) struct ImeState {
    pub(crate) marked_text: String,
}

/// 光标闪烁间隔。
const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(500);

/// 终端视图。
pub struct TerminalView {
    focus_handle: FocusHandle,
    terminal: Option<Entity<Terminal>>,
    subscription: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
    error: Option<String>,
    settings: Arc<TerminalRenderSettings>,
    title: SharedString,
    /// IME 组合文本状态（由 `TerminalElement` 在渲染时读取）。
    pub(crate) ime_state: Option<ImeState>,
    /// 光标闪烁相位。
    cursor_phase: bool,
}

impl TerminalView {
    /// 创建终端视图并异步启动终端（PTY + 事件循环）。
    pub fn new(
        working_directory: Option<PathBuf>,
        shell: Shell,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        let settings = Arc::new(TerminalRenderSettings::default());

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
                        this.terminal = Some(terminal);
                        this.subscription = Some(subscription);
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
            settings,
            title: "终端".into(),
            ime_state: None,
            cursor_phase: true,
        }
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
            TerminalEvent::CloseTerminal => cx.quit(),
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

            let handled =
                terminal.update(cx, |terminal, _cx| terminal.try_keystroke(&e.keystroke, false));
            if handled {
                cx.stop_propagation();
            }
        }
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
        // 仅当窗口中没有任何元素持有焦点时（应用启动 / 焦点真空）才接管焦点。
        // 不能无条件抢占：设置弹窗打开期间，对话框与其中的输入框持有焦点，而终端
        // 会随 PTY Wakeup / 光标闪烁不断重渲染，若在此抢焦点会导致：
        //   1. 弹窗内输入框无法保持焦点、无法输入；
        //   2. 弹窗右上角关闭按钮（通过焦点路径分发 Cancel 动作）点击无效。
        // 点击终端区域时 TerminalElement 的左键 on_mouse_down 会自行聚焦。
        if window.focused(cx).is_none() {
            window.focus(&self.focus_handle, cx);
        }

        window.set_window_title(&self.title);

        let focused = self.focus_handle.is_focused(window);
        let cursor_visible = self.should_show_cursor(focused, cx);

        let mut root = div()
            .id("terminal-view")
            .size_full()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
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
