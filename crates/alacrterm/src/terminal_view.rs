use std::{ops::Range, path::PathBuf, time::Duration};

use alacritty_terminal::{
    index::{Column, Direction, Line, Point as AlacPoint},
    selection::SelectionType,
};
use gpui::{
    Bounds, ClipboardItem, Context, Entity, EntityInputHandler, FocusHandle, IntoElement,
    KeyContext, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Render,
    ScrollWheelEvent, Subscription, UTF16Selection, Window, div, point, prelude::*, px, size,
};
use gpui_component::Theme as ComponentTheme;
use terminal::{Terminal, TerminalBounds, TerminalTheme};

use crate::settings::{BlinkMode, TerminalSettings};
use crate::terminal_element::TerminalElement;

/// 终端视图 — 持有 Entity<Terminal>，处理输入并渲染 TerminalElement
pub struct TerminalView {
    terminal: Entity<Terminal>,
    focus_handle: FocusHandle,
    cursor_visible: bool,
    blinking_terminal_enabled: bool,
    was_focused: bool,
    is_selecting: bool,
    font_size: f32,
    line_height: f32,
    cell_width: f32,
    settings: TerminalSettings,
    /// 当前滚动偏移（每帧从 content.display_offset 更新），用于鼠标坐标转换
    display_offset: usize,
    terminal_bounds: TerminalBounds,
    context_menu: Option<ContextMenuState>,
    hover_target: Option<HoverTarget>,
    pub(crate) ime_marked_text: Option<String>,
    scrollbar_drag: Option<ScrollbarDrag>,
    /// 子像素滚动累积量（像素），用于平滑触控板滚动
    scroll_px: f32,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone, Debug)]
struct HoverTarget {
    text: String,
    kind: HoverKind,
    point: AlacPoint,
}

#[derive(Clone, Debug)]
enum HoverKind {
    Url(String),
    Path(PathTarget),
}

#[derive(Clone, Debug)]
struct PathTarget {
    display: String,
    path: PathBuf,
    line: Option<usize>,
    column: Option<usize>,
}

#[derive(Clone, Copy, Debug)]
struct ContextMenuState {
    position: gpui::Point<gpui::Pixels>,
    opened_from: AlacPoint,
}

#[derive(Clone, Copy, Debug)]
struct ScrollbarDrag {
    grab_offset_y: f32,
}

impl TerminalView {
    const MIN_FONT_SIZE: f32 = 9.0;
    const MAX_FONT_SIZE: f32 = 28.0;
    const PADDING: f32 = 4.0;

    /// 将窗口像素坐标转换为终端绝对网格坐标
    /// 视觉行 R → 绝对行 R - display_offset（滚动后历史行为负数）
    fn pixel_to_point(&self, x: f32, y: f32) -> Option<AlacPoint> {
        let origin = self.terminal_bounds.bounds.origin;
        let origin_x: f32 = origin.x.into();
        let origin_y: f32 = origin.y.into();
        let width: f32 = self.terminal_bounds.bounds.size.width.into();
        let height: f32 = self.terminal_bounds.bounds.size.height.into();
        if x < origin_x || y < origin_y || x >= origin_x + width || y >= origin_y + height {
            return None;
        }

        let max_col = self.terminal_bounds.num_columns().saturating_sub(1);
        let max_row = self.terminal_bounds.num_lines().saturating_sub(1) as i32;
        let col = (((x - origin_x) / self.cell_width).floor().max(0.0) as usize).min(max_col);
        let visual_row = (((y - origin_y) / self.line_height).floor().max(0.0) as i32).min(max_row);
        let line = visual_row - self.display_offset as i32;
        Some(AlacPoint::new(Line(line), Column(col)))
    }

    /// 根据 x 坐标在格子内的位置决定选区侧（左半 = Left，右半 = Right）
    fn pixel_side(&self, x: f32) -> Direction {
        let origin_x: f32 = self.terminal_bounds.bounds.origin.x.into();
        let frac = ((x - origin_x) / self.cell_width).fract();
        if frac < 0.5 {
            Direction::Left
        } else {
            Direction::Right
        }
    }

    fn set_font_size(&mut self, font_size: f32) {
        self.font_size = font_size.clamp(Self::MIN_FONT_SIZE, Self::MAX_FONT_SIZE);
        self.settings.font_size = self.font_size;
        self.line_height = self.settings.line_height();
    }

    fn zoom_in(&mut self) {
        self.set_font_size(self.font_size + 1.0);
    }

    fn zoom_out(&mut self) {
        self.set_font_size(self.font_size - 1.0);
    }

    fn reset_zoom(&mut self) {
        self.set_font_size(TerminalSettings::default().font_size);
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
                "a" => {
                    self.terminal.update(cx, |terminal, cx| {
                        terminal.select_all();
                        terminal.sync(cx);
                    });
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
                "f" => {
                    if let Some(query) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        self.terminal.update(cx, |terminal, cx| {
                            terminal.set_search_query(query);
                            terminal.sync(cx);
                        });
                        cx.notify();
                        return true;
                    }
                }
                _ => {}
            }
        }

        if key == "escape" {
            self.terminal.update(cx, |terminal, cx| {
                terminal.clear_search();
                terminal.sync(cx);
            });
            cx.notify();
            return true;
        }

        if modifiers.shift && key == "insert" {
            return self.paste_from_clipboard(cx);
        }

        false
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let terminal = cx.new(|cx| Terminal::new(cx).expect("Failed to create terminal"));
        let settings = TerminalSettings::default();
        let focus_handle = cx.focus_handle();

        // 订阅终端事件 → 触发 TerminalView 重新渲染
        let subscriptions = vec![
            cx.subscribe(&terminal, |_this, _term, event, cx| {
                use terminal::Event;
                match event {
                    Event::Wakeup | Event::TitleChanged => cx.notify(),
                    Event::Bell => { /* TODO: 视觉响铃 */ }
                    Event::CloseTerminal => { /* TODO: 关闭窗口 */ }
                    Event::SelectionsChanged => cx.notify(),
                    Event::CursorBlinkingChanged(blinking) => {
                        _this.blinking_terminal_enabled = *blinking;
                        if !blinking {
                            _this.cursor_visible = true;
                        }
                        cx.notify();
                    }
                }
            }),
            cx.on_focus_in(&focus_handle, window, |this, window, cx| {
                this.was_focused = true;
                this.cursor_visible = true;
                this.terminal
                    .update(cx, |terminal, _cx| terminal.focus_in());
                window.invalidate_character_coordinates();
                cx.notify();
            }),
            cx.on_focus_out(&focus_handle, window, |this, _event, window, cx| {
                this.was_focused = false;
                this.context_menu = None;
                this.is_selecting = false;
                this.terminal
                    .update(cx, |terminal, _cx| terminal.focus_out());
                window.invalidate_character_coordinates();
                cx.notify();
            }),
        ];

        // 光标闪烁：每 500ms 切换一次可见性
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                let _ = this.update(cx, |view, cx| {
                    let blink_enabled = match view.settings.blinking {
                        BlinkMode::Off => false,
                        BlinkMode::On => true,
                        BlinkMode::TerminalControlled => view.blinking_terminal_enabled,
                    };
                    if blink_enabled {
                        view.cursor_visible = !view.cursor_visible;
                        cx.notify();
                    }
                });
            }
        })
        .detach();

        Self {
            terminal,
            focus_handle,
            cursor_visible: true,
            blinking_terminal_enabled: false,
            was_focused: false,
            is_selecting: false,
            font_size: settings.font_size,
            line_height: settings.line_height(),
            cell_width: settings.font_size * 0.6,
            settings,
            display_offset: 0,
            terminal_bounds: TerminalBounds::default(),
            context_menu: None,
            hover_target: None,
            ime_marked_text: None,
            scrollbar_drag: None,
            scroll_px: 0.0,
            _subscriptions: subscriptions,
        }
    }

    fn scrollbar_track_bounds(&self) -> (f32, f32) {
        let origin_y: f32 = self.terminal_bounds.bounds.origin.y.into();
        let height: f32 = self.terminal_bounds.bounds.size.height.into();
        (origin_y, height.max(self.line_height))
    }

    fn scrollbar_thumb(&self, history_size: usize, display_offset: usize) -> (f32, f32) {
        let (_track_top, track_height) = self.scrollbar_track_bounds();
        let visible_lines = self.terminal_bounds.num_lines().max(1) as f32;
        let history_size = history_size as f32;
        let total = visible_lines + history_size;
        let thumb_height =
            (track_height * (visible_lines / total.max(visible_lines))).clamp(24.0, track_height);
        let max_top = (track_height - thumb_height).max(0.0);
        let top = if history_size <= 0.0 {
            0.0
        } else {
            ((history_size - display_offset.min(history_size as usize) as f32) / history_size)
                .clamp(0.0, 1.0)
                * max_top
        };
        (top, thumb_height)
    }

    fn display_offset_for_thumb_top(
        &self,
        thumb_top: f32,
        thumb_height: f32,
        history_size: usize,
    ) -> usize {
        let (_track_top, track_height) = self.scrollbar_track_bounds();
        let max_top = (track_height - thumb_height).max(0.0);
        if history_size == 0 || max_top <= 0.0 {
            return 0;
        }
        let progress = (thumb_top / max_top).clamp(0.0, 1.0);
        ((1.0 - progress) * history_size as f32).round() as usize
    }

    fn terminal_theme(cx: &mut Context<Self>) -> TerminalTheme {
        let colors = ComponentTheme::global(cx).colors;
        TerminalTheme::new(
            colors.foreground,
            colors.background,
            colors.caret,
            colors.selection,
            [
                colors.background,
                colors.red,
                colors.green,
                colors.yellow,
                colors.blue,
                colors.magenta,
                colors.cyan,
                colors.foreground,
                colors.muted_foreground,
                colors.red_light,
                colors.green_light,
                colors.yellow_light,
                colors.blue_light,
                colors.magenta_light,
                colors.cyan_light,
                colors.popover_foreground,
            ],
        )
    }

    fn dispatch_context(&self, content: &terminal::TerminalContent) -> KeyContext {
        use alacritty_terminal::term::TermMode;

        let mut context = KeyContext::new_with_defaults();
        context.add("Terminal");
        context.set(
            "screen",
            if content.mode.contains(TermMode::ALT_SCREEN) {
                "alt"
            } else {
                "normal"
            },
        );
        if content.mode.contains(TermMode::APP_CURSOR) {
            context.add("DECCKM");
        }
        if content.mode.contains(TermMode::APP_KEYPAD) {
            context.add("DECPAM");
        } else {
            context.add("DECPNM");
        }
        if content.mode.contains(TermMode::BRACKETED_PASTE) {
            context.add("bracketed_paste");
        }
        if content.mode.contains(TermMode::ALTERNATE_SCROLL) {
            context.add("alternate_scroll");
        }
        if content.mode.intersects(TermMode::MOUSE_MODE) {
            context.add("any_mouse_reporting");
        }
        let mouse_reporting = if content.mode.contains(TermMode::MOUSE_REPORT_CLICK) {
            "click"
        } else if content.mode.contains(TermMode::MOUSE_DRAG) {
            "drag"
        } else if content.mode.contains(TermMode::MOUSE_MOTION) {
            "motion"
        } else {
            "off"
        };
        context.set("mouse_reporting", mouse_reporting);
        context.set(
            "mouse_format",
            if content.mode.contains(TermMode::SGR_MOUSE) {
                "sgr"
            } else if content.mode.contains(TermMode::UTF8_MOUSE) {
                "utf8"
            } else {
                "normal"
            },
        );
        if content.selection.is_some() {
            context.add("selection");
        }
        context
    }

    fn target_at(&mut self, point: AlacPoint, cx: &mut Context<Self>) -> Option<HoverTarget> {
        if let Some(uri) = self
            .terminal
            .update(cx, |terminal, _cx| terminal.hyperlink_at(point))
        {
            return Some(HoverTarget {
                text: uri.clone(),
                kind: HoverKind::Url(uri),
                point,
            });
        }

        let content = self
            .terminal
            .update(cx, |terminal, _cx| terminal.last_content.clone());
        let row: Vec<_> = content
            .cells
            .iter()
            .filter(|cell| cell.point.line == point.line)
            .collect();
        let cell_index = row
            .iter()
            .position(|cell| cell.point.column == point.column)?;

        let is_target_char = |ch: char| {
            ch.is_ascii_alphanumeric()
                || matches!(
                    ch,
                    ':' | '/' | '\\' | '.' | '_' | '-' | '~' | '?' | '&' | '=' | '#' | '%'
                )
        };

        let mut start = cell_index;
        while start > 0 && is_target_char(row[start - 1].c) {
            start -= 1;
        }
        let mut end = cell_index;
        while end + 1 < row.len() && is_target_char(row[end + 1].c) {
            end += 1;
        }

        let text: String = row[start..=end].iter().map(|cell| cell.c).collect();
        if is_url_like(&text) {
            Some(HoverTarget {
                text: text.clone(),
                kind: HoverKind::Url(text),
                point,
            })
        } else if let Some(path) = self.resolve_path_target(&text, cx) {
            Some(HoverTarget {
                text: path.display.clone(),
                kind: HoverKind::Path(path),
                point,
            })
        } else {
            None
        }
    }

    fn resolve_path_target(&mut self, text: &str, cx: &mut Context<Self>) -> Option<PathTarget> {
        let parsed = parse_path_target(text)?;
        let cwd = self
            .terminal
            .update(cx, |terminal, _cx| terminal.working_directory())
            .or_else(|| std::env::current_dir().ok())?;
        let path = if parsed.path.is_absolute() {
            parsed.path
        } else {
            cwd.join(parsed.path)
        };

        Some(PathTarget {
            display: format_path_target(&path, parsed.line, parsed.column),
            path,
            line: parsed.line,
            column: parsed.column,
        })
    }

    fn ime_cursor_bounds(&self, cx: &mut Context<Self>) -> Bounds<gpui::Pixels> {
        let cursor = self
            .terminal
            .update(cx, |terminal, _cx| terminal.last_content.cursor)
            .map(|cursor| cursor.point)
            .unwrap_or_else(|| AlacPoint::new(Line(0), Column(0)));
        let display_row = cursor.line.0 + self.display_offset as i32;
        Bounds {
            origin: self.terminal_bounds.bounds.origin
                + point(
                    px(cursor.column.0 as f32 * self.cell_width),
                    px(display_row.max(0) as f32 * self.line_height),
                ),
            size: size(px(self.cell_width), px(self.line_height)),
        }
    }

    pub(crate) fn update_terminal_layout(&mut self, bounds: TerminalBounds, display_offset: usize) {
        self.cell_width = bounds.cell_width;
        self.line_height = bounds.line_height;
        self.display_offset = display_offset;
        self.terminal_bounds = bounds;
    }
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<gpui::Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(0)
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.ime_marked_text
            .as_ref()
            .map(|text| 0..text.encode_utf16().count())
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.ime_marked_text = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ime_marked_text = None;
        if !new_text.is_empty() {
            self.terminal.update(cx, |terminal, _cx| {
                terminal.input(new_text.as_bytes().to_vec())
            });
        }
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ime_marked_text = (!new_text.is_empty()).then(|| new_text.to_string());
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _element_bounds: Bounds<gpui::Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<gpui::Pixels>> {
        Some(self.ime_cursor_bounds(_cx))
    }
}

fn is_url_like(text: &str) -> bool {
    text.starts_with("http://") || text.starts_with("https://")
}

struct ParsedPathTarget {
    path: PathBuf,
    line: Option<usize>,
    column: Option<usize>,
}

fn parse_path_target(text: &str) -> Option<ParsedPathTarget> {
    let trimmed =
        text.trim_matches(|ch: char| matches!(ch, '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';'));
    if trimmed.is_empty() || is_url_like(trimmed) {
        return None;
    }

    let (path_text, line, column) = split_line_column(trimmed);
    if path_text.is_empty() {
        return None;
    }
    let looks_like_path = path_text.contains('/')
        || path_text.contains('\\')
        || path_text.starts_with('.')
        || path_text.ends_with(".rs")
        || path_text.ends_with(".toml")
        || path_text.ends_with(".json")
        || path_text.ends_with(".md");
    looks_like_path.then(|| ParsedPathTarget {
        path: PathBuf::from(path_text),
        line,
        column,
    })
}

fn split_line_column(text: &str) -> (&str, Option<usize>, Option<usize>) {
    let mut path_end = text.len();
    let mut numbers = Vec::new();
    for _ in 0..2 {
        let Some(colon) = text[..path_end].rfind(':') else {
            break;
        };
        let suffix = &text[colon + 1..path_end];
        let Ok(number) = suffix.parse::<usize>() else {
            break;
        };
        numbers.push(number);
        path_end = colon;
    }

    let column = if numbers.len() == 2 {
        numbers.first().copied()
    } else {
        None
    };
    let line = numbers.last().copied();
    (&text[..path_end], line, column)
}

fn format_path_target(
    path: &std::path::Path,
    line: Option<usize>,
    column: Option<usize>,
) -> String {
    let mut text = path.display().to_string();
    if let Some(line) = line {
        text.push(':');
        text.push_str(&line.to_string());
        if let Some(column) = column {
            text.push(':');
            text.push_str(&column.to_string());
        }
    }
    text
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let terminal_theme = Self::terminal_theme(cx);
        let content = self
            .terminal
            .update(cx, |terminal, _cx| terminal.last_content.clone());

        // ── 构建 UI ──────────────────────────────────────────────────────────
        let focused = self.focus_handle.is_focused(window);
        if !focused && self.context_menu.is_some() {
            self.context_menu = None;
        }
        if focused != self.was_focused {
            self.was_focused = focused;
            self.terminal.update(cx, |terminal, _cx| {
                if focused {
                    terminal.focus_in();
                } else {
                    terminal.focus_out();
                }
            });
        }
        let blink_enabled = match self.settings.blinking {
            BlinkMode::Off => false,
            BlinkMode::On => true,
            BlinkMode::TerminalControlled => self.blinking_terminal_enabled,
        };
        let cursor_visible = !focused || !blink_enabled || self.cursor_visible;
        let key_context = self.dispatch_context(&content);
        let menu = self.context_menu;
        let hover = self.hover_target.clone();
        let terminal_view = cx.entity();
        let scrollbar = self.render_scrollbar(
            content.history_size,
            content.display_offset,
            terminal_theme,
            cx,
        );

        let mut root = div()
            .id("terminal-view")
            .key_context(key_context)
            .track_focus(&self.focus_handle)
            .size_full()
            .relative()
            .bg(terminal_theme.background)
            .p(px(Self::PADDING))
            // ── 鼠标点击获取焦点 + 开始选区 ─────────────────────────────────
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus_handle, cx);
                    this.context_menu = None;
                    let x: f32 = event.position.x.into();
                    let y: f32 = event.position.y.into();
                    let Some(point) = this.pixel_to_point(x, y) else {
                        this.is_selecting = false;
                        this.hover_target = None;
                        cx.notify();
                        return;
                    };
                    if event.modifiers.platform || event.modifiers.control {
                        if let Some(target) = this.target_at(point, cx) {
                            match target.kind {
                                HoverKind::Url(uri) => cx.open_url(&uri),
                                HoverKind::Path(path) => {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        format_path_target(&path.path, path.line, path.column),
                                    ));
                                }
                            }
                            cx.stop_propagation();
                            return;
                        }
                    }
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
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus_handle, cx);
                    let x: f32 = _event.position.x.into();
                    let y: f32 = _event.position.y.into();
                    if let Some(point) = this.pixel_to_point(x, y) {
                        this.context_menu = Some(ContextMenuState {
                            position: _event.position,
                            opened_from: point,
                        });
                    } else {
                        this.context_menu = None;
                    }
                    cx.notify();
                    cx.stop_propagation();
                }),
            )
            // ── 鼠标拖动更新选区 ─────────────────────────────────────────────
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                let x: f32 = event.position.x.into();
                let y: f32 = event.position.y.into();
                if let Some(drag) = this.scrollbar_drag {
                    let (thumb_top, thumb_height) = {
                        let content = this
                            .terminal
                            .update(cx, |terminal, _cx| terminal.last_content.clone());
                        let (track_top, _track_height) = this.scrollbar_track_bounds();
                        let target_top = y - track_top - drag.grab_offset_y;
                        let (_, thumb_height) =
                            this.scrollbar_thumb(content.history_size, content.display_offset);
                        (target_top, thumb_height)
                    };
                    this.terminal.update(cx, |terminal, _cx| {
                        let target = this.display_offset_for_thumb_top(
                            thumb_top,
                            thumb_height,
                            terminal.last_content.history_size,
                        );
                        terminal.scroll_to_display_offset(target);
                    });
                    cx.notify();
                    return;
                }
                let Some(point) = this.pixel_to_point(x, y) else {
                    if this.hover_target.is_some() {
                        this.hover_target = None;
                        cx.notify();
                    }
                    return;
                };
                let next_hover = this.target_at(point, cx);
                if next_hover.as_ref().map(|hover| (&hover.text, hover.point))
                    != this
                        .hover_target
                        .as_ref()
                        .map(|hover| (&hover.text, hover.point))
                {
                    this.hover_target = next_hover;
                    cx.notify();
                }
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
                    if this.scrollbar_drag.take().is_some() {
                        cx.notify();
                        cx.stop_propagation();
                        return;
                    }
                    let x: f32 = event.position.x.into();
                    let y: f32 = event.position.y.into();
                    let Some(point) = this.pixel_to_point(x, y) else {
                        this.is_selecting = false;
                        return;
                    };
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
                    terminal.try_keystroke(&event.keystroke, this.settings.option_as_meta)
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
                        // 子像素精度：累积像素量，满一行才触发滚动
                        let dy: f32 = p.y.into();
                        this.scroll_px += dy * this.settings.scroll_multiplier;
                        let full_lines = (this.scroll_px / this.line_height) as i32;
                        this.scroll_px -= full_lines as f32 * this.line_height;
                        full_lines
                    }
                    ScrollDelta::Lines(p) => {
                        // 行级滚动直接计算，重置子像素累积
                        this.scroll_px = 0.0;
                        let dy: f32 = p.y.into();
                        (dy * 3.0 * this.settings.scroll_multiplier).round() as i32
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
                            if let Some(point) = point {
                                for _ in 0..lines {
                                    handled |= terminal.mouse_scroll_up(point);
                                }
                            }
                            if !handled {
                                terminal.scroll_up_by(lines as usize);
                            }
                        } else {
                            let mut handled = false;
                            if let Some(point) = point {
                                for _ in 0..-lines {
                                    handled |= terminal.mouse_scroll_down(point);
                                }
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
            .child(TerminalElement::new(
                self.terminal.clone(),
                terminal_theme,
                self.settings.font_family.clone(),
                self.font_size,
                self.line_height,
                self.settings.cursor_shape,
                self.settings.minimum_contrast,
                focused,
                cursor_visible,
                terminal_view,
                self.focus_handle.clone(),
                self.settings.font_weight,
                self.ime_marked_text.clone(),
            ))
            .child(scrollbar);

        if let Some(hover) = hover {
            root = root.child(render_hover_target(hover, terminal_theme));
        }
        if let Some(menu) = menu {
            root = root.child(self.render_context_menu(menu, terminal_theme, cx));
        }
        root
    }
}

impl TerminalView {
    fn render_context_menu(
        &self,
        state: ContextMenuState,
        theme: TerminalTheme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let left: f32 = state.position.x.into();
        let top: f32 = state.position.y.into();
        let opened_from = state.opened_from;
        div()
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(160.0))
            .bg(theme.background)
            .border_1()
            .border_color(theme.selection)
            .text_color(theme.foreground)
            .child(self.context_menu_item(
                "Copy",
                cx.listener(|this, _, _window, cx| {
                    this.copy_selection(cx);
                    this.context_menu = None;
                    cx.notify();
                }),
            ))
            .child(self.context_menu_item(
                "Paste",
                cx.listener(|this, _, _window, cx| {
                    this.paste_from_clipboard(cx);
                    this.context_menu = None;
                    cx.notify();
                }),
            ))
            .child(self.context_menu_item(
                "Select All",
                cx.listener(|this, _, _window, cx| {
                    this.terminal.update(cx, |terminal, cx| {
                        terminal.select_all();
                        terminal.sync(cx);
                    });
                    this.context_menu = None;
                    cx.notify();
                }),
            ))
            .child(self.context_menu_item(
                "Clear",
                cx.listener(|this, _, _window, cx| {
                    this.terminal.update(cx, |terminal, _cx| terminal.clear());
                    this.context_menu = None;
                    cx.notify();
                }),
            ))
            .child(self.context_menu_item(
                "Copy Target",
                cx.listener(move |this, _, _window, cx| {
                    if let Some(target) = this.target_at(opened_from, cx) {
                        cx.write_to_clipboard(ClipboardItem::new_string(target.text));
                    }
                    this.context_menu = None;
                    cx.notify();
                }),
            ))
    }

    fn context_menu_item(
        &self,
        label: &'static str,
        listener: impl Fn(&MouseDownEvent, &mut Window, &mut gpui::App) + 'static,
    ) -> impl IntoElement {
        div()
            .px(px(10.0))
            .py(px(6.0))
            .hover(|style| style.bg(gpui::black().opacity(0.12)))
            .on_mouse_down(MouseButton::Left, listener)
            .child(label)
    }

    fn render_scrollbar(
        &self,
        history_size: usize,
        display_offset: usize,
        theme: TerminalTheme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let (_track_top, track_height) = self.scrollbar_track_bounds();
        let (thumb_top, thumb_height) = self.scrollbar_thumb(history_size, display_offset);

        div()
            .absolute()
            .right(px(2.0))
            .top(px(Self::PADDING))
            .h(px(track_height))
            .w(px(8.0))
            .bg(theme.background)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    let y: f32 = event.position.y.into();
                    let (track_top, _) = this.scrollbar_track_bounds();
                    let (thumb_top, thumb_height) = this.scrollbar_thumb(
                        this.terminal
                            .update(cx, |terminal, _cx| terminal.last_content.history_size),
                        this.terminal
                            .update(cx, |terminal, _cx| terminal.last_content.display_offset),
                    );
                    let local_y = y - track_top;
                    if local_y >= thumb_top && local_y <= thumb_top + thumb_height {
                        this.scrollbar_drag = Some(ScrollbarDrag {
                            grab_offset_y: local_y - thumb_top,
                        });
                    } else {
                        let target_top = local_y - thumb_height / 2.0;
                        let target = this.display_offset_for_thumb_top(
                            target_top,
                            thumb_height,
                            this.terminal
                                .update(cx, |terminal, _cx| terminal.last_content.history_size),
                        );
                        this.terminal.update(cx, |terminal, _cx| {
                            terminal.scroll_to_display_offset(target);
                        });
                        this.scrollbar_drag = Some(ScrollbarDrag {
                            grab_offset_y: thumb_height / 2.0,
                        });
                    }
                    cx.notify();
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .absolute()
                    .top(px(thumb_top))
                    .right(px(1.0))
                    .w(px(6.0))
                    .h(px(thumb_height))
                    .rounded(px(3.0))
                    .bg(theme.selection),
            )
    }
}

fn render_hover_target(hover: HoverTarget, theme: TerminalTheme) -> impl IntoElement {
    div()
        .absolute()
        .left(px(8.0))
        .bottom(px(8.0))
        .max_w(px(720.0))
        .px(px(8.0))
        .py(px(4.0))
        .bg(theme.selection)
        .text_color(theme.foreground)
        .child(hover.text)
}
