use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use alacritty_terminal::{
    index::{Column, Direction, Line, Point as AlacPoint},
    selection::SelectionType,
};
use gpui::{
    Bounds, Context, Entity, FocusHandle, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Render, ScrollWheelEvent,
    Subscription, Window, div, prelude::*, px, rgb,
};
use gpui_component::ElementExt as _;
use terminal::{Terminal, TerminalBounds};

use crate::terminal_element::TerminalElement;

/// 终端视图 — 持有 Entity<Terminal>，处理输入并渲染 TerminalElement
pub struct TerminalView {
    terminal: Entity<Terminal>,
    focus_handle: FocusHandle,
    cursor_visible: bool,
    is_selecting: bool,
    /// 当前滚动偏移（每帧从 content.display_offset 更新），用于鼠标坐标转换
    display_offset: usize,
    /// 终端元素在窗口中的真实位置和尺寸，由 on_prepaint 每帧更新
    element_bounds: Rc<RefCell<Bounds<Pixels>>>,
    _subscriptions: Vec<Subscription>,
}

impl TerminalView {
    const FONT_SIZE: f32 = 14.0;
    const LINE_HEIGHT: f32 = 20.0;
    const CELL_WIDTH: f32 = Self::FONT_SIZE * 0.6;
    const PADDING: f32 = 4.0;

    /// 将窗口像素坐标转换为终端绝对网格坐标
    /// origin_x/y 为终端元素左上角在窗口中的实际坐标（由 on_prepaint 获取）
    fn pixel_to_point(x: f32, y: f32, origin_x: f32, origin_y: f32, display_offset: usize) -> AlacPoint {
        let col = ((x - origin_x - Self::PADDING) / Self::CELL_WIDTH)
            .floor()
            .max(0.0) as usize;
        let visual_row = ((y - origin_y - Self::PADDING) / Self::LINE_HEIGHT)
            .floor()
            .max(0.0) as i32;
        let line = visual_row - display_offset as i32;
        AlacPoint::new(Line(line), Column(col))
    }

    /// 根据 x 坐标在格子内的位置决定选区侧（左半 = Left，右半 = Right）
    fn pixel_side(x: f32, origin_x: f32) -> Direction {
        let frac = ((x - origin_x - Self::PADDING) / Self::CELL_WIDTH).fract();
        if frac < 0.5 { Direction::Left } else { Direction::Right }
    }
    pub fn new(cx: &mut Context<Self>) -> Self {
        let terminal = cx.new(|cx| {
            Terminal::new(cx).expect("Failed to create terminal")
        });

        // 订阅终端事件 → 触发 TerminalView 重新渲染
        let subscriptions = vec![
            cx.subscribe(&terminal, |_this, _term, event, cx| {
                use terminal::Event;
                match event {
                    Event::Wakeup | Event::TitleChanged => cx.notify(),
                    Event::Bell => { /* TODO: 视觉响铃 */ }
                    Event::CloseTerminal => { /* TODO: 关闭窗口 */ }
                    Event::SelectionsChanged => cx.notify(),
                }
            }),
        ];

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
            display_offset: 0,
            element_bounds: Rc::new(RefCell::new(Bounds::default())),
            _subscriptions: subscriptions,
        }
    }
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 用上一帧 on_prepaint 捕获的实际元素尺寸来计算终端行列数
        // 首帧尺寸为 0，会在第二帧（光标闪烁触发）后稳定
        let stored = *self.element_bounds.borrow();
        let elem_w: f32 = stored.size.width.into();
        let elem_h: f32 = stored.size.height.into();
        let available_w = if elem_w > 0.0 { elem_w } else { 200.0 };
        let available_h = if elem_h > 0.0 { elem_h } else { 200.0 };

        let new_bounds = TerminalBounds::new(
            Self::CELL_WIDTH,
            Self::LINE_HEIGHT,
            (available_w - Self::PADDING * 2.0).max(Self::CELL_WIDTH),
            (available_h - Self::PADDING * 2.0).max(Self::LINE_HEIGHT),
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
        let focused = self.focus_handle.is_focused(_window);
        let cursor_visible = self.cursor_visible;

        // 克隆 Rc，传入 on_prepaint 用于每帧更新元素位置和尺寸
        let bounds_rc = self.element_bounds.clone();

        div()
            .id("terminal-view")
            .key_context("Terminal")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(rgb(0x1a1b26))
            .p(px(Self::PADDING))
            // ── 每帧记录元素在窗口中的真实位置和尺寸 ────────────────────────
            .on_prepaint(move |bounds, _, _| {
                *bounds_rc.borrow_mut() = bounds;
            })
            // ── 鼠标点击获取焦点 + 开始选区 ─────────────────────────────────
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus_handle, cx);
                    let b = *this.element_bounds.borrow();
                    let ox: f32 = b.origin.x.into();
                    let oy: f32 = b.origin.y.into();
                    let x: f32 = event.position.x.into();
                    let y: f32 = event.position.y.into();
                    let point = TerminalView::pixel_to_point(x, y, ox, oy, this.display_offset);
                    let side = TerminalView::pixel_side(x, ox);
                    this.is_selecting = true;
                    this.terminal.update(cx, |terminal, _cx| {
                        terminal.start_selection(SelectionType::Simple, point, side);
                    });
                    cx.notify();
                    cx.stop_propagation();
                }),
            )
            // ── 鼠标拖动更新选区 ─────────────────────────────────────────────
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.is_selecting
                    && event.pressed_button == Some(MouseButton::Left)
                {
                    let b = *this.element_bounds.borrow();
                    let ox: f32 = b.origin.x.into();
                    let oy: f32 = b.origin.y.into();
                    let x: f32 = event.position.x.into();
                    let y: f32 = event.position.y.into();
                    let point = TerminalView::pixel_to_point(x, y, ox, oy, this.display_offset);
                    let side = TerminalView::pixel_side(x, ox);
                    this.terminal.update(cx, |terminal, _cx| {
                        terminal.update_selection(point, side);
                    });
                    cx.notify();
                }
            }))
            // ── 松开鼠标结束选区 ─────────────────────────────────────────────
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, _cx| {
                    this.is_selecting = false;
                }),
            )
            // ── 键盘输入 ─────────────────────────────────────────────────────
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                let handled = this.terminal.update(cx, |terminal, _cx| {
                    terminal.try_keystroke(&event.keystroke, false)
                });
                if handled {
                    cx.stop_propagation();
                }
            }))
            // ── 滚轮（向上滚动 dy > 0，向下滚动 dy < 0）─────────────────────
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
                use gpui::ScrollDelta;
                let lines = match &event.delta {
                    ScrollDelta::Pixels(p) => {
                        let dy: f32 = p.y.into();
                        (dy / Self::LINE_HEIGHT).round() as i32
                    }
                    ScrollDelta::Lines(p) => {
                        let dy: f32 = p.y.into();
                        (dy * 3.0).round() as i32
                    }
                };
                if lines != 0 {
                    this.terminal.update(cx, |terminal, _cx| {
                        if lines > 0 {
                            terminal.scroll_up_by(lines as usize);
                        } else {
                            terminal.scroll_down_by((-lines) as usize);
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
