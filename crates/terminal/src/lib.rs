pub mod mappings;
mod pty_info;
mod utils;

pub use mappings::{colors::TerminalTheme, keys::to_esc_str};
pub use pty_info::{ProcessInfo, PtyProcessInfo};

use alacritty_terminal::{
    Term,
    event::{Event as AlacEvent, EventListener, WindowSize},
    event_loop::{EventLoop, Msg, Notifier, State},
    grid::{Dimensions, Scroll as AlacScroll},
    index::{Column, Direction as AlacDirection, Point as AlacPoint},
    selection::{Selection, SelectionRange, SelectionType},
    sync::FairMutex,
    term::{Config, RenderableCursor, TermMode, cell::Cell, search::RegexSearch},
    tty,
};
use mappings::mouse::{MouseReport, MouseReportKind, encode_mouse_report};

use gpui::{Bounds, ClipboardItem, Context, EventEmitter, Keystroke, Pixels, Point, Size, px};
use std::{borrow::Cow, collections::VecDeque, sync::Arc, thread::JoinHandle};

// ─── 尺寸 ────────────────────────────────────────────────────────────────────

/// 终端像素尺寸（参考 Zed 的 TerminalBounds）
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalBounds {
    pub cell_width: f32,
    pub line_height: f32,
    pub bounds: Bounds<Pixels>,
}

impl Default for TerminalBounds {
    fn default() -> Self {
        Self {
            cell_width: 8.0,
            line_height: 16.0,
            bounds: Bounds {
                origin: Point::default(),
                size: Size {
                    width: px(640.0),
                    height: px(384.0),
                },
            },
        }
    }
}

impl TerminalBounds {
    pub fn new(cell_width: f32, line_height: f32, bounds: Bounds<Pixels>) -> Self {
        Self {
            cell_width,
            line_height,
            bounds,
        }
    }

    pub fn width(&self) -> f32 {
        self.bounds.size.width.into()
    }

    pub fn height(&self) -> f32 {
        self.bounds.size.height.into()
    }

    pub fn num_lines(&self) -> usize {
        let raw = self.height() / self.line_height;
        // Match Zed: compensate for f32 precision without allocating partial rows.
        raw.next_up().floor().max(1.0) as usize
    }

    pub fn num_columns(&self) -> usize {
        let raw = self.width() / self.cell_width;
        raw.next_up().floor().max(1.0) as usize
    }
}

impl From<TerminalBounds> for WindowSize {
    fn from(b: TerminalBounds) -> Self {
        WindowSize {
            num_lines: b.num_lines() as u16,
            num_cols: b.num_columns() as u16,
            cell_width: b.cell_width as u16,
            cell_height: b.line_height as u16,
        }
    }
}

struct TermSize {
    columns: usize,
    screen_lines: usize,
}

impl Dimensions for TermSize {
    fn columns(&self) -> usize {
        self.columns
    }
    fn screen_lines(&self) -> usize {
        self.screen_lines
    }
    fn total_lines(&self) -> usize {
        self.screen_lines + 10_000
    }
}

// ─── 单元格 ───────────────────────────────────────────────────────────────────

/// 带位置信息的单元格（参考 Zed 的 IndexedCell）
#[derive(Clone, Debug)]
pub struct IndexedCell {
    pub point: AlacPoint,
    pub cell: Cell,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchMatch {
    pub start: AlacPoint,
    pub end: AlacPoint,
}

impl std::ops::Deref for IndexedCell {
    type Target = Cell;
    fn deref(&self) -> &Cell {
        &self.cell
    }
}

// ─── 终端内容快照 ──────────────────────────────────────────────────────────────

/// 渲染所需的终端内容快照（参考 Zed 的 TerminalContent）
#[derive(Clone, Default)]
pub struct TerminalContent {
    /// 可见区域的所有单元格（包含样式信息）
    pub cells: Vec<IndexedCell>,
    /// 终端模式标志（用于键盘映射、鼠标等）
    pub mode: TermMode,
    /// 当前显示偏移（滚动位置）
    pub display_offset: usize,
    /// 滚回历史行数
    pub history_size: usize,
    /// 选中的文本范围
    pub selection: Option<SelectionRange>,
    /// 选中的文本内容
    pub selection_text: Option<String>,
    /// 光标信息
    pub cursor: Option<RenderableCursor>,
    /// 光标所在位置的字符
    pub cursor_char: char,
    /// 终端像素尺寸
    pub terminal_bounds: TerminalBounds,
    /// 标题文字
    pub title: String,
    /// 是否滚动到了顶部
    pub scrolled_to_top: bool,
    /// 是否滚动到了底部
    pub scrolled_to_bottom: bool,
    /// 当前搜索匹配范围
    pub search_matches: Vec<SearchMatch>,
}

// ─── 事件 ─────────────────────────────────────────────────────────────────────

/// 终端向 TerminalView 发出的事件
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// 内容更新，需要重新渲染
    Wakeup,
    /// 终端响铃
    Bell,
    /// 标题变更
    TitleChanged,
    /// 终端进程退出，请求关闭
    CloseTerminal,
    /// 选中内容变化
    SelectionsChanged,
    /// 终端请求改变光标闪烁状态
    CursorBlinkingChanged(bool),
}

// ─── 内部事件队列 ──────────────────────────────────────────────────────────────

enum InternalEvent {
    Resize(TerminalBounds),
    Scroll(AlacScroll),
    SetSelection(Option<(Selection, AlacPoint)>),
    UpdateSelection(AlacPoint, alacritty_terminal::index::Side),
    MouseReport(MouseReport),
    Clear,
    /// 仅展示模式：原始 VTE 字节序列（通过 write_output 注入）
    WriteOutput(Vec<u8>),
}

// ─── Alacritty 事件监听器 ──────────────────────────────────────────────────────

#[derive(Clone)]
struct TermListener {
    events_tx: async_channel::Sender<AlacEvent>,
}

impl EventListener for TermListener {
    fn send_event(&self, event: AlacEvent) {
        let _ = self.events_tx.try_send(event);
    }
}

// ─── Terminal ─────────────────────────────────────────────────────────────────

/// 终端后端运行模式
enum TerminalType {
    /// 连接真实 PTY shell 的终端
    Pty {
        pty_tx: Notifier,
        _io_thread: JoinHandle<(EventLoop<tty::Pty, TermListener>, State)>,
        pty_info: Arc<PtyProcessInfo>,
    },
    /// 仅展示模式：无 PTY，通过 write_output() 注入内容
    DisplayOnly,
}

/// 终端后端（GPUI Model），管理 PTY、alacritty term 和内容渲染
pub struct Terminal {
    term: Arc<FairMutex<Term<TermListener>>>,
    terminal_type: TerminalType,
    /// 最近一次同步的渲染内容快照
    pub last_content: TerminalContent,
    /// 待处理的内部事件队列
    events: VecDeque<InternalEvent>,
    /// 标题文字
    breadcrumb_text: String,
    /// 当前 UI 主题映射到终端 ANSI/特殊颜色的快照
    pub theme: TerminalTheme,
    search_query: Option<String>,
}

/// 向后兼容：pty_info 通过方法暴露
impl Terminal {
    pub fn pty_info(&self) -> Option<&Arc<PtyProcessInfo>> {
        match &self.terminal_type {
            TerminalType::Pty { pty_info, .. } => Some(pty_info),
            TerminalType::DisplayOnly => None,
        }
    }
}

impl Terminal {
    /// 创建终端（在 cx.new() 中调用）
    pub fn new(cx: &mut Context<Self>) -> anyhow::Result<Self> {
        let (events_tx, events_rx) = async_channel::unbounded::<AlacEvent>();

        let listener = TermListener { events_tx };

        let shell = utils::find_shell();
        let pty_config = tty::Options {
            shell,
            working_directory: None,
            drain_on_exit: true,
            env: std::collections::HashMap::new(),
            #[cfg(target_os = "windows")]
            escape_args: true,
        };

        let bounds = TerminalBounds::default();
        let config = Config {
            scrolling_history: 10_000,
            ..Config::default()
        };

        let size = TermSize {
            columns: bounds.num_columns(),
            screen_lines: bounds.num_lines(),
        };

        let pty = tty::new(&pty_config, bounds.into(), 0)?;
        let pty_info = Arc::new(PtyProcessInfo::new(&pty));
        let term = Term::new(config, &size, listener.clone());
        let term = Arc::new(FairMutex::new(term));

        let event_loop = EventLoop::new(term.clone(), listener, pty, true, false)?;
        let pty_tx = Notifier(event_loop.channel());
        let io_thread = event_loop.spawn();

        cx.spawn(async move |this, cx| {
            while let Ok(event) = events_rx.recv().await {
                let _ = this.update(cx, |terminal, cx| {
                    terminal.drain_alac_events(vec![event], cx);
                });

                cx.background_executor()
                    .timer(std::time::Duration::from_millis(4))
                    .await;

                let mut batch = Vec::new();
                while batch.len() < 100 {
                    match events_rx.try_recv() {
                        Ok(event) => batch.push(event),
                        Err(_) => break,
                    }
                }

                if !batch.is_empty() {
                    let _ = this.update(cx, |terminal, cx| {
                        terminal.drain_alac_events(batch, cx);
                    });
                }
            }
        })
        .detach();

        Ok(Self {
            term,
            terminal_type: TerminalType::Pty {
                pty_tx,
                _io_thread: io_thread,
                pty_info,
            },
            last_content: TerminalContent::default(),
            events: VecDeque::new(),
            breadcrumb_text: String::new(),
            theme: TerminalTheme::default(),
            search_query: None,
        })
    }

    /// 创建仅展示模式的终端（无 PTY，通过 write_output 注入内容）
    pub fn new_display_only(_cx: &mut Context<Self>) -> Self {
        let (events_tx, _events_rx) = async_channel::unbounded::<AlacEvent>();
        let listener = TermListener { events_tx };
        let bounds = TerminalBounds::default();
        let config = Config {
            scrolling_history: 10_000,
            ..Config::default()
        };
        let size = TermSize {
            columns: bounds.num_columns(),
            screen_lines: bounds.num_lines(),
        };
        let term = Term::new(config, &size, listener);
        let term = Arc::new(FairMutex::new(term));

        Self {
            term,
            terminal_type: TerminalType::DisplayOnly,
            last_content: TerminalContent::default(),
            events: VecDeque::new(),
            breadcrumb_text: String::new(),
            theme: TerminalTheme::default(),
            search_query: None,
        }
    }

    /// 消费 alacritty 事件并处理
    fn drain_alac_events(&mut self, events: Vec<AlacEvent>, cx: &mut Context<Self>) {
        let mut needs_sync = false;
        for event in events {
            match event {
                AlacEvent::Wakeup => {
                    needs_sync = true;
                }
                AlacEvent::Bell => {
                    cx.emit(Event::Bell);
                }
                AlacEvent::Title(title) => {
                    self.breadcrumb_text = title;
                    cx.emit(Event::TitleChanged);
                }
                AlacEvent::ResetTitle => {
                    self.breadcrumb_text.clear();
                    cx.emit(Event::TitleChanged);
                }
                AlacEvent::PtyWrite(data) => {
                    self.write_to_pty(data.into_bytes());
                }
                AlacEvent::ClipboardStore(_, text) => {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
                AlacEvent::ClipboardLoad(_, format) => {
                    let text = cx
                        .read_from_clipboard()
                        .and_then(|item| item.text())
                        .unwrap_or_default();
                    self.write_to_pty(format(&text).into_bytes());
                }
                AlacEvent::ColorRequest(index, format) => {
                    let color = mappings::colors::to_vte_rgb(self.theme.color_at_index(index));
                    self.write_to_pty(format(color).into_bytes());
                }
                AlacEvent::TextAreaSizeRequest(format) => {
                    self.write_to_pty(
                        format(self.last_content.terminal_bounds.into()).into_bytes(),
                    );
                }
                AlacEvent::CursorBlinkingChange => {
                    let blinking = self.term.lock().cursor_style().blinking;
                    cx.emit(Event::CursorBlinkingChanged(blinking));
                }
                AlacEvent::Exit | AlacEvent::ChildExit(_) => {
                    cx.emit(Event::CloseTerminal);
                }
                _ => {
                    needs_sync = true;
                }
            }
        }

        if needs_sync {
            self.sync(cx);
            cx.emit(Event::Wakeup);
        }
    }

    /// 处理内部事件并刷新内容快照
    pub fn sync(&mut self, _cx: &mut Context<Self>) {
        let term = self.term.clone();
        let mut terminal = term.lock_unfair();

        let mut selection_changed = false;
        while let Some(event) = self.events.pop_front() {
            match event {
                InternalEvent::Resize(bounds) => {
                    terminal.resize(TermSize {
                        columns: bounds.num_columns(),
                        screen_lines: bounds.num_lines(),
                    });
                    if let TerminalType::Pty { pty_tx, .. } = &self.terminal_type {
                        let _ = pty_tx.0.send(Msg::Resize(bounds.into()));
                    }
                }
                InternalEvent::Scroll(scroll) => {
                    terminal.scroll_display(scroll);
                }
                InternalEvent::SetSelection(sel) => {
                    terminal.selection = sel.map(|(s, _)| s);
                    selection_changed = true;
                }
                InternalEvent::UpdateSelection(point, side) => {
                    if let Some(mut sel) = terminal.selection.take() {
                        sel.update(point, side);
                        terminal.selection = Some(sel);
                        selection_changed = true;
                    }
                }
                InternalEvent::MouseReport(report) => {
                    if let Some(bytes) = encode_mouse_report(
                        report,
                        *terminal.mode(),
                        self.last_content.display_offset,
                    ) {
                        drop(terminal);
                        if let TerminalType::Pty { pty_tx, .. } = &self.terminal_type {
                            let _ = pty_tx.0.send(Msg::Input(Cow::Owned(bytes)));
                        }
                        terminal = term.lock_unfair();
                    }
                }
                InternalEvent::Clear => {
                    // 发送清屏转义序列到 PTY（清除屏幕 + 清除滚回）
                    drop(terminal);
                    if let TerminalType::Pty { pty_tx, .. } = &self.terminal_type {
                        let _ = pty_tx
                            .0
                            .send(Msg::Input(Cow::Borrowed(b"\x1b[2J\x1b[3J\x1b[H")));
                    }
                    terminal = term.lock_unfair();
                }
                InternalEvent::WriteOutput(data) => {
                    // DisplayOnly 模式：使用 alacritty_terminal 内置的 VTE 处理器处理字节
                    use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
                    let mut processor: Processor<StdSyncHandler> = Processor::new();
                    processor.advance(&mut *terminal, &data);
                }
            }
        }

        let content = terminal.renderable_content();
        let cursor = content.cursor;
        let cursor_char = terminal.grid()[cursor.point].c;
        let display_offset = content.display_offset;
        let history_size = terminal.history_size();

        let mut cells = Vec::new();
        for ic in content.display_iter {
            cells.push(IndexedCell {
                point: ic.point,
                cell: ic.cell.clone(),
            });
        }

        let selection_text = if content.selection.is_some() {
            terminal.selection_to_string()
        } else {
            None
        };
        let search_matches = self
            .search_query
            .as_deref()
            .map(|query| collect_search_matches(&terminal, query))
            .unwrap_or_default();

        self.last_content = TerminalContent {
            cells,
            mode: content.mode,
            display_offset,
            history_size,
            selection: content.selection,
            selection_text,
            cursor: Some(cursor),
            cursor_char,
            terminal_bounds: self.last_content.terminal_bounds,
            title: self.breadcrumb_text.clone(),
            scrolled_to_top: display_offset == history_size,
            scrolled_to_bottom: display_offset == 0,
            search_matches,
        };

        if selection_changed {
            _cx.emit(Event::SelectionsChanged);
        }
    }

    // ─── 公开 API ──────────────────────────────────────────────────────────────

    /// 发送字节到 PTY
    pub fn input(&mut self, data: impl Into<Cow<'static, [u8]>>) {
        self.write_to_pty(data.into());
    }

    /// 处理键盘事件，返回是否已处理
    pub fn try_keystroke(&mut self, keystroke: &Keystroke, option_as_meta: bool) -> bool {
        let esc = to_esc_str(keystroke, &self.last_content.mode, option_as_meta);
        if let Some(esc) = esc {
            self.events
                .push_back(InternalEvent::Scroll(AlacScroll::Bottom));
            match esc {
                Cow::Borrowed(s) => self.input(s.as_bytes()),
                Cow::Owned(s) => self.input(s.into_bytes()),
            }
            true
        } else {
            false
        }
    }

    /// 调整终端大小
    pub fn resize(&mut self, new_bounds: TerminalBounds) {
        let old = &self.last_content.terminal_bounds;
        let needs_resize = old.num_lines() != new_bounds.num_lines()
            || old.num_columns() != new_bounds.num_columns();

        self.last_content.terminal_bounds = new_bounds;

        if needs_resize {
            match self.events.back_mut() {
                Some(InternalEvent::Resize(pending)) => {
                    *pending = new_bounds;
                }
                _ => {
                    self.events.push_back(InternalEvent::Resize(new_bounds));
                }
            }
        }
    }

    pub fn set_theme(&mut self, theme: TerminalTheme) {
        self.theme = theme;
    }

    pub fn set_search_query(&mut self, query: impl Into<String>) {
        let query = query.into();
        self.search_query = (!query.is_empty()).then_some(query);
    }

    pub fn clear_search(&mut self) {
        self.search_query = None;
    }

    pub fn hyperlink_at(&self, point: AlacPoint) -> Option<String> {
        self.last_content
            .cells
            .iter()
            .find(|cell| cell.point == point)
            .and_then(|cell| cell.cell.hyperlink())
            .map(|hyperlink| hyperlink.uri().to_string())
    }

    pub fn working_directory(&self) -> Option<std::path::PathBuf> {
        match &self.terminal_type {
            TerminalType::Pty { pty_info, .. } => {
                pty_info.current.as_ref().map(|info| info.cwd.clone())
            }
            TerminalType::DisplayOnly => None,
        }
    }

    /// 粘贴文本（支持 bracketed paste 模式）
    pub fn paste(&mut self, text: &str) {
        let paste_text = if self.last_content.mode.contains(TermMode::BRACKETED_PASTE) {
            format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', ""))
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r")
        };
        self.input(paste_text.into_bytes());
    }

    /// 复制当前选中内容
    pub fn copy(&self) -> Option<String> {
        self.term.lock().selection_to_string()
    }

    pub fn has_selection(&self) -> bool {
        self.last_content.selection.is_some()
    }

    /// 向上滚动若干行
    pub fn scroll_up_by(&mut self, lines: usize) {
        if self.should_send_alternate_scroll() {
            for _ in 0..lines.max(1) {
                self.input(b"\x1b[A".as_slice());
            }
            return;
        }
        self.events
            .push_back(InternalEvent::Scroll(AlacScroll::Delta(lines as i32)));
    }

    /// 向下滚动若干行
    pub fn scroll_down_by(&mut self, lines: usize) {
        if self.should_send_alternate_scroll() {
            for _ in 0..lines.max(1) {
                self.input(b"\x1b[B".as_slice());
            }
            return;
        }
        self.events
            .push_back(InternalEvent::Scroll(AlacScroll::Delta(-(lines as i32))));
    }

    pub fn mouse_mode(&self) -> bool {
        self.last_content.mode.intersects(TermMode::MOUSE_MODE)
    }

    pub fn mouse_down(&mut self, button: u8, point: AlacPoint) -> bool {
        if !self.mouse_mode() {
            return false;
        }
        self.events
            .push_back(InternalEvent::MouseReport(MouseReport {
                point,
                kind: MouseReportKind::Press(button),
            }));
        true
    }

    pub fn mouse_up(&mut self, button: u8, point: AlacPoint) -> bool {
        if !self.mouse_mode() {
            return false;
        }
        self.events
            .push_back(InternalEvent::MouseReport(MouseReport {
                point,
                kind: MouseReportKind::Release(button),
            }));
        true
    }

    pub fn mouse_move(&mut self, button: u8, point: AlacPoint) -> bool {
        let mode = self.last_content.mode;
        if !mode.contains(TermMode::MOUSE_DRAG) && !mode.contains(TermMode::MOUSE_MOTION) {
            return false;
        }
        self.events
            .push_back(InternalEvent::MouseReport(MouseReport {
                point,
                kind: MouseReportKind::Move(button),
            }));
        true
    }

    pub fn mouse_scroll_up(&mut self, point: AlacPoint) -> bool {
        if !self.mouse_mode() {
            return false;
        }
        self.events
            .push_back(InternalEvent::MouseReport(MouseReport {
                point,
                kind: MouseReportKind::ScrollUp,
            }));
        true
    }

    pub fn mouse_scroll_down(&mut self, point: AlacPoint) -> bool {
        if !self.mouse_mode() {
            return false;
        }
        self.events
            .push_back(InternalEvent::MouseReport(MouseReport {
                point,
                kind: MouseReportKind::ScrollDown,
            }));
        true
    }

    /// 向上翻一页
    pub fn scroll_page_up(&mut self) {
        self.events
            .push_back(InternalEvent::Scroll(AlacScroll::PageUp));
    }

    /// 向下翻一页
    pub fn scroll_page_down(&mut self) {
        self.events
            .push_back(InternalEvent::Scroll(AlacScroll::PageDown));
    }

    /// 滚动到顶部
    pub fn scroll_to_top(&mut self) {
        self.events
            .push_back(InternalEvent::Scroll(AlacScroll::Top));
    }

    /// 滚动到底部
    pub fn scroll_to_bottom(&mut self) {
        self.events
            .push_back(InternalEvent::Scroll(AlacScroll::Bottom));
    }

    pub fn scroll_to_display_offset(&mut self, display_offset: usize) {
        let current = self.last_content.display_offset as i32;
        let target = display_offset.min(self.last_content.history_size) as i32;
        let delta = target - current;
        if delta != 0 {
            self.events
                .push_back(InternalEvent::Scroll(AlacScroll::Delta(delta)));
        }
    }

    /// 清除屏幕
    pub fn clear(&mut self) {
        self.events.push_back(InternalEvent::Clear);
    }

    /// 开始选择
    pub fn start_selection(
        &mut self,
        selection_type: SelectionType,
        point: AlacPoint,
        side: alacritty_terminal::index::Direction,
    ) {
        let selection = Selection::new(selection_type, point, side);
        self.events
            .push_back(InternalEvent::SetSelection(Some((selection, point))));
    }

    /// 更新选择终点
    /// side: 鼠标在格子内的位置（Left = 左半格，Right = 右半格）
    pub fn update_selection(&mut self, point: AlacPoint, side: alacritty_terminal::index::Side) {
        self.events
            .push_back(InternalEvent::UpdateSelection(point, side));
    }

    /// 清除选择
    pub fn clear_selection(&mut self) {
        self.events.push_back(InternalEvent::SetSelection(None));
    }

    pub fn select_all(&mut self) {
        let terminal = self.term.lock();
        let mut selection = Selection::new(
            SelectionType::Lines,
            AlacPoint::new(terminal.topmost_line(), Column(0)),
            AlacDirection::Left,
        );
        selection.update(
            AlacPoint::new(terminal.bottommost_line(), terminal.last_column()),
            AlacDirection::Right,
        );
        drop(terminal);
        self.events.push_back(InternalEvent::SetSelection(Some((
            selection,
            AlacPoint::new(alacritty_terminal::index::Line(0), Column(0)),
        ))));
    }

    pub fn focus_in(&mut self) {
        let mut terminal = self.term.lock();
        terminal.is_focused = true;
        let report_focus = terminal.mode().contains(TermMode::FOCUS_IN_OUT);
        drop(terminal);
        if report_focus {
            self.input(b"\x1b[I".as_slice());
        }
    }

    pub fn focus_out(&mut self) {
        let mut terminal = self.term.lock();
        terminal.is_focused = false;
        let report_focus = terminal.mode().contains(TermMode::FOCUS_IN_OUT);
        drop(terminal);
        if report_focus {
            self.input(b"\x1b[O".as_slice());
        }
    }

    /// 获取终端标题
    pub fn title(&self) -> &str {
        if self.breadcrumb_text.is_empty() {
            "Terminal"
        } else {
            &self.breadcrumb_text
        }
    }

    fn write_to_pty(&self, data: impl Into<Cow<'static, [u8]>>) {
        if let TerminalType::Pty { pty_tx, .. } = &self.terminal_type {
            let _ = pty_tx.0.send(Msg::Input(data.into()));
        }
    }

    /// 仅展示模式：将原始 VTE 字节直接注入终端网格（不经过 PTY）
    /// 这些字节会在 sync() 时通过 InternalEvent::WriteOutput 处理
    pub fn write_output(&mut self, data: impl Into<Vec<u8>>) {
        // DisplayOnly 下直接推入事件队列
        if matches!(self.terminal_type, TerminalType::DisplayOnly) {
            self.events
                .push_back(InternalEvent::WriteOutput(data.into()));
        }
        // PTY 模式下此方法无效（输出应从 PTY 自然读取）
    }

    /// 返回是否为仅展示模式
    pub fn is_display_only(&self) -> bool {
        matches!(self.terminal_type, TerminalType::DisplayOnly)
    }

    fn should_send_alternate_scroll(&self) -> bool {
        self.last_content.mode.contains(TermMode::ALT_SCREEN)
            && self.last_content.mode.contains(TermMode::ALTERNATE_SCROLL)
            && !self.mouse_mode()
    }
}

fn collect_search_matches(term: &Term<TermListener>, query: &str) -> Vec<SearchMatch> {
    let Ok(mut regex) = RegexSearch::new(&regex_escape(query)) else {
        return Vec::new();
    };

    let start = AlacPoint::new(term.topmost_line(), Column(0));
    let end = AlacPoint::new(term.bottommost_line(), term.last_column());
    alacritty_terminal::term::search::RegexIter::new(
        start,
        end,
        AlacDirection::Right,
        term,
        &mut regex,
    )
    .map(|range| SearchMatch {
        start: *range.start(),
        end: *range.end(),
    })
    .collect()
}

fn regex_escape(query: &str) -> String {
    let mut escaped = String::with_capacity(query.len());
    for ch in query.chars() {
        if matches!(
            ch,
            '.' | '+' | '*' | '?' | '^' | '$' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '\\'
        ) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

impl EventEmitter<Event> for Terminal {}

impl Drop for Terminal {
    fn drop(&mut self) {
        if let TerminalType::Pty { pty_tx, .. } = &self.terminal_type {
            let _ = pty_tx.0.send(Msg::Shutdown);
        }
    }
}
