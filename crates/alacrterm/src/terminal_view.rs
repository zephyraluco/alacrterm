use std::time::Duration;

use alacritty_terminal::{
    index::{Column, Direction, Line, Point as AlacPoint},
    selection::SelectionType,
};
use gpui::{
    ClipboardItem, Context, Entity, FocusHandle, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Render, ScrollWheelEvent, Subscription, Window,
    div, prelude::*, px, rgb,
};
use terminal::{Terminal, TerminalBounds};

use crate::terminal_element::TerminalElement;

/// 终端视图 — 持有 Entity<Terminal>，处理输入并渲染 TerminalElement
pub struct TerminalView {
    terminal: Entity<Terminal>,
    focus_handle: FocusHandle,
    cursor_visible: bool,
    is_selecting: bool,
    font_size: f32,
    line_height: f32,
    cell_width: f32,
    /// 当前滚动偏移（每帧从 content.display_offset 更新），用于鼠标坐标转换
    display_offset: usize,
    _subscriptions: Vec<Subscription>,
}

impl TerminalView {
    const FONT_SIZE: f32 = 14.0;
    const MIN_FONT_SIZE: f32 = 9.0;
    const MAX_FONT_SIZE: f32 = 28.0;
    const LINE_HEIGHT_RATIO: f32 = 20.0 / 14.0;
    const CELL_WIDTH_RATIO: f32 = 0.6;
    const PADDING: f32 = 4.0;
    /// gpui-component TitleBar 固定高度（见 gpui-component title_bar.rs TITLE_BAR_HEIGHT）
    const TITLE_BAR_HEIGHT: f32 = 34.0;

    /// 将窗口像素坐标转换为终端绝对网格坐标
    /// 视觉行 R → 绝对行 R - display_offset（滚动后历史行为负数）
    fn pixel_to_point(&self, x: f32, y: f32) -> AlacPoint {
        let col = ((x - Self::PADDING) / self.cell_width).floor().max(0.0) as usize;
        let visual_row = ((y - Self::TITLE_BAR_HEIGHT - Self::PADDING) / self.line_height)
            .floor()
            .max(0.0) as i32;
        let line = visual_row - self.display_offset as i32;
        AlacPoint::new(Line(line), Column(col))
    }

    /// 根据 x 坐标在格子内的位置决定选区侧（左半 = Left，右半 = Right）
    fn pixel_side(&self, x: f32) -> Direction {
        let frac = ((x - Self::PADDING) / self.cell_width).fract();
        if frac < 0.5 {
            Direction::Left
        } else {
            Direction::Right
        }
    }

    fn set_font_size(&mut self, font_size: f32) {
        self.font_size = font_size.clamp(Self::MIN_FONT_SIZE, Self::MAX_FONT_SIZE);
        self.line_height = self.font_size * Self::LINE_HEIGHT_RATIO;
        self.cell_width = self.font_size * Self::CELL_WIDTH_RATIO;
    }

    fn zoom_in(&mut self) {
        self.set_font_size(self.font_size + 1.0);
    }

    fn zoom_out(&mut self) {
        self.set_font_size(self.font_size - 1.0);
    }

    fn reset_zoom(&mut self) {
        self.set_font_size(Self::FONT_SIZE);
    }

    fn selection_type_for_click_count(click_count: usize) -> SelectionType {
        match click_count {
            0 | 1 => SelectionType::Simple,
            2 => SelectionType::Semantic,
            _ => SelectionType::Lines,
        }
    }

    fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let text = self.terminal.update(cx, |terminal, cx| {
            terminal.sync(cx);
            terminal.copy()
        });
        if let Some(text) = text.filter(|text| !text.is_empty()) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.terminal
                .update(cx, |terminal, _cx| terminal.clear_selection());
            cx.notify();
            true
        } else {
            false
        }
    }

    fn paste_from_clipboard(&mut self, cx: &mut Context<Self>) -> bool {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.terminal
                .update(cx, |terminal, _cx| terminal.paste(&text));
            self.cursor_visible = true;
            cx.notify();
            true
        } else {
            false
        }
    }

    fn handle_editor_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let key = event.keystroke.key.as_str();
        let modifiers = &event.keystroke.modifiers;
        let command = modifiers.platform || modifiers.control;

        if command {
            match key {
                "+" | "=" | "plus" => {
                    self.zoom_in();
                    cx.notify();
                    return true;
                }
                "-" | "minus" => {
                    self.zoom_out();
                    cx.notify();
                    return true;
                }
                "0" => {
                    self.reset_zoom();
                    cx.notify();
                    return true;
                }
                "c" if modifiers.shift || modifiers.platform => return self.copy_selection(cx),
                "c" if modifiers.control => {
                    let has_selection = self
                        .terminal
                        .update(cx, |terminal, _cx| terminal.has_selection());
                    if has_selection {
                        return self.copy_selection(cx);
                    }
                }
                "v" if modifiers.shift || modifiers.platform || modifiers.control => {
                    return self.paste_from_clipboard(cx);
                }
                _ => {}
            }
        }

        if modifiers.shift && key == "insert" {
            return self.paste_from_clipboard(cx);
        }

        false
    }

    pub fn new(cx: &mut Context<Self>) -> Self {
        let terminal = cx.new(|cx| Terminal::new(cx).expect("Failed to create terminal"));

        // 订阅终端事件 → 触发 TerminalView 重新渲染
        let subscriptions = vec![cx.subscribe(&terminal, |_this, _term, event, cx| {
            use terminal::Event;
            match event {
                Event::Wakeup | Event::TitleChanged => cx.notify(),
                Event::Bell => { /* TODO: 视觉响铃 */ }
                Event::CloseTerminal => { /* TODO: 关闭窗口 */ }
                Event::SelectionsChanged => cx.notify(),
            }
        })];

        // 光标闪烁：每 500ms 切换一次可见性
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                let _ = this.update(cx, |view, cx| {
                    view.cursor_visible = !view.cursor_visible;
                    cx.notify();
                });
            }
        })
        .detach();

        Self {
            terminal,
            focus_handle: cx.focus_handle(),
            cursor_visible: true,
            is_selecting: false,
            font_size: Self::FONT_SIZE,
            line_height: Self::FONT_SIZE * Self::LINE_HEIGHT_RATIO,
            cell_width: Self::FONT_SIZE * Self::CELL_WIDTH_RATIO,
            display_offset: 0,
            _subscriptions: subscriptions,
        }
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let vp = window.viewport_size();
        let available_w: f32 = vp.width.into();
        let available_h: f32 = vp.height.into();

        let new_bounds = TerminalBounds::new(
            self.cell_width,
            self.line_height,
            (available_w - Self::PADDING * 2.0).max(self.cell_width),
            (available_h - Self::TITLE_BAR_HEIGHT - Self::PADDING * 2.0).max(self.line_height),
        );

        // ── 同步终端尺寸并获取内容快照 ─────────────────────────────────────────
        let content = self.terminal.update(cx, |terminal, cx| {
            terminal.resize(new_bounds);
            terminal.sync(cx);
            terminal.last_content.clone()
        });
        // 缓存滚动偏移，供鼠标事件坐标转换使用
        self.display_offset = content.display_offset;

        // ── 构建 UI ──────────────────────────────────────────────────────────
        let focused = self.focus_handle.is_focused(window);
        let cursor_visible = self.cursor_visible;

        div()
            .id("terminal-view")
            .key_context("Terminal")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(rgb(0x1a1b26))
            .p(px(Self::PADDING))
            // ── 鼠标点击获取焦点 + 开始选区 ─────────────────────────────────
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus_handle, cx);
                    let x: f32 = event.position.x.into();
                    let y: f32 = event.position.y.into();
                    let point = this.pixel_to_point(x, y);
                    let handled = this
                        .terminal
                        .update(cx, |terminal, _cx| terminal.mouse_down(0, point));
                    this.cursor_visible = true;
                    if handled {
                        cx.notify();
                        cx.stop_propagation();
                        return;
                    }

                    let side = this.pixel_side(x);
                    this.is_selecting = true;
                    let selection_type = Self::selection_type_for_click_count(event.click_count);
                    this.terminal.update(cx, |terminal, _cx| {
                        terminal.start_selection(selection_type, point, side);
                    });
                    cx.notify();
                    cx.stop_propagation();
                }),
            )
            // ── 鼠标拖动更新选区 ─────────────────────────────────────────────
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                let x: f32 = event.position.x.into();
                let y: f32 = event.position.y.into();
                let point = this.pixel_to_point(x, y);
                let button = if event.pressed_button == Some(MouseButton::Left) {
                    0
                } else {
                    3
                };
                let reported = this
                    .terminal
                    .update(cx, |terminal, _cx| terminal.mouse_move(button, point));
                if reported {
                    cx.notify();
                    return;
                }

                if this.is_selecting && event.pressed_button == Some(MouseButton::Left) {
                    let side = this.pixel_side(x);
                    this.terminal.update(cx, |terminal, _cx| {
                        terminal.update_selection(point, side);
                    });
                    cx.notify();
                }
            }))
            // ── 松开鼠标结束选区 ─────────────────────────────────────────────
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _window, cx| {
                    let x: f32 = event.position.x.into();
                    let y: f32 = event.position.y.into();
                    let point = this.pixel_to_point(x, y);
                    let handled = this
                        .terminal
                        .update(cx, |terminal, _cx| terminal.mouse_up(0, point));
                    this.is_selecting = false;
                    if handled {
                        cx.notify();
                        cx.stop_propagation();
                    }
                }),
            )
            // ── 键盘输入 ─────────────────────────────────────────────────────
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                if this.handle_editor_key(event, cx) {
                    cx.stop_propagation();
                    return;
                }

                let handled = this.terminal.update(cx, |terminal, _cx| {
                    terminal.try_keystroke(&event.keystroke, false)
                });
                if handled {
                    this.cursor_visible = true;
                    cx.stop_propagation();
                }
            }))
            // ── 滚轮（向上滚动 dy > 0，向下滚动 dy < 0）─────────────────────
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
                use gpui::ScrollDelta;
                let lines = match &event.delta {
                    ScrollDelta::Pixels(p) => {
                        let dy: f32 = p.y.into();
                        (dy / this.line_height).round() as i32
                    }
                    ScrollDelta::Lines(p) => {
                        let dy: f32 = p.y.into();
                        (dy * 3.0).round() as i32
                    }
                };
                if event.modifiers.control || event.modifiers.platform {
                    if lines > 0 {
                        this.zoom_in();
                    } else if lines < 0 {
                        this.zoom_out();
                    }
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
                if lines != 0 {
                    let x: f32 = event.position.x.into();
                    let y: f32 = event.position.y.into();
                    let point = this.pixel_to_point(x, y);
                    this.terminal.update(cx, |terminal, _cx| {
                        if lines > 0 {
                            let mut handled = false;
                            for _ in 0..lines {
                                handled |= terminal.mouse_scroll_up(point);
                            }
                            if !handled {
                                terminal.scroll_up_by(lines as usize);
                            }
                        } else {
                            let mut handled = false;
                            for _ in 0..-lines {
                                handled |= terminal.mouse_scroll_down(point);
                            }
                            if !handled {
                                terminal.scroll_down_by((-lines) as usize);
                            }
                        }
                    });
                    cx.notify();
                }
                cx.stop_propagation();
            }))
            // ── 终端内容 ─────────────────────────────────────────────────────
            .child(TerminalElement::new(content, focused, cursor_visible).render_div())
    }
}
