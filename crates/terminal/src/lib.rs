pub mod mappings;
mod pty_info;
mod utils;

pub use mappings::keys::to_esc_str;
pub use pty_info::{ProcessInfo, PtyProcessInfo};

use alacritty_terminal::{
    Term,
    event::{Event as AlacEvent, EventListener, WindowSize},
    event_loop::{EventLoop, Msg, Notifier, State},
    grid::{Dimensions, Scroll as AlacScroll},
    index::Point as AlacPoint,
    selection::{Selection, SelectionRange, SelectionType},
    sync::FairMutex,
    term::{Config, RenderableCursor, TermMode, cell::Cell},
    tty,
};

use gpui::{Context, EventEmitter, Keystroke};
use std::{
    borrow::Cow,
    collections::VecDeque,
    sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}},
    thread::JoinHandle,
};

// ─── 尺寸 ────────────────────────────────────────────────────────────────────

/// 终端像素尺寸（参考 Zed 的 TerminalBounds）
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalBounds {
    pub cell_width: f32,
    pub line_height: f32,
    pub width: f32,
    pub height: f32,
}

impl Default for TerminalBounds {
    fn default() -> Self {
        Self {
            cell_width: 8.0,
            line_height: 16.0,
            width: 640.0,
            height: 384.0,
        }
    }
}

impl TerminalBounds {
    pub fn new(cell_width: f32, line_height: f32, width: f32, height: f32) -> Self {
        Self { cell_width, line_height, width, height }
    }

    pub fn num_lines(&self) -> usize {
        let raw = self.height / self.line_height;
        // floor: 只分配完整的行，避免最后一行被截断，也避免亚像素抖动触发不必要的 resize
        raw.floor().max(1.0) as usize
    }

    pub fn num_columns(&self) -> usize {
        let raw = self.width / self.cell_width;
        raw.ceil().max(1.0) as usize
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
    fn columns(&self) -> usize { self.columns }
    fn screen_lines(&self) -> usize { self.screen_lines }
    fn total_lines(&self) -> usize { self.screen_lines + 10_000 }
}

// ─── 单元格 ───────────────────────────────────────────────────────────────────

/// 带位置信息的单元格（参考 Zed 的 IndexedCell）
#[derive(Clone, Debug)]
pub struct IndexedCell {
    pub point: AlacPoint,
    pub cell: Cell,
}

impl std::ops::Deref for IndexedCell {
    type Target = Cell;
    fn deref(&self) -> &Cell { &self.cell }
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
}

// ─── 内部事件队列 ──────────────────────────────────────────────────────────────

enum InternalEvent {
    Resize(TerminalBounds),
    Scroll(AlacScroll),
    SetSelection(Option<(Selection, AlacPoint)>),
    UpdateSelection(AlacPoint, alacritty_terminal::index::Side),
    Clear,
}

// ─── Alacritty 事件监听器 ──────────────────────────────────────────────────────

#[derive(Clone)]
struct TermListener {
    dirty: Arc<AtomicBool>,
    events: Arc<Mutex<VecDeque<AlacEvent>>>,
}

impl EventListener for TermListener {
    fn send_event(&self, event: AlacEvent) {
        let mut q = self.events.lock().unwrap();
        q.push_back(event);
        self.dirty.store(true, Ordering::Release);
    }
}

// ─── Terminal ─────────────────────────────────────────────────────────────────

/// 终端后端（GPUI Model），管理 PTY、alacritty term 和内容渲染
pub struct Terminal {
    term: Arc<FairMutex<Term<TermListener>>>,
    pty_tx: Notifier,
    _io_thread: JoinHandle<(EventLoop<tty::Pty, TermListener>, State)>,
    /// 最近一次同步的渲染内容快照
    pub last_content: TerminalContent,
    /// 待处理的内部事件队列
    events: VecDeque<InternalEvent>,
    /// 用于通知 GPUI 有更新的脏标志
    #[allow(dead_code)]
    dirty: Arc<AtomicBool>,
    /// alacritty 事件队列
    alac_events: Arc<Mutex<VecDeque<AlacEvent>>>,
    /// 标题文字
    breadcrumb_text: String,
    /// PTY 进程信息
    pub pty_info: Arc<PtyProcessInfo>,
}

impl Terminal {
    /// 创建终端（在 cx.new() 中调用）
    pub fn new(cx: &mut Context<Self>) -> anyhow::Result<Self> {
        let dirty = Arc::new(AtomicBool::new(false));
        let alac_events = Arc::new(Mutex::new(VecDeque::<AlacEvent>::new()));

        let listener = TermListener {
            dirty: dirty.clone(),
            events: alac_events.clone(),
        };

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

        // 启动轮询任务：每 8ms 检查一次脏标志
        let dirty_clone = dirty.clone();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(8))
                    .await;

                if dirty_clone.swap(false, Ordering::Acquire) {
                    let _ = this.update(cx, |terminal, cx| {
                        terminal.drain_alac_events(cx);
                    });
                }
            }
        })
        .detach();

        Ok(Self {
            term,
            pty_tx,
            _io_thread: io_thread,
            last_content: TerminalContent::default(),
            events: VecDeque::new(),
            dirty,
            alac_events,
            breadcrumb_text: String::new(),
            pty_info,
        })
    }

    /// 消费 alacritty 事件并处理
    fn drain_alac_events(&mut self, cx: &mut Context<Self>) {
        let events: Vec<AlacEvent> = {
            let mut q = self.alac_events.lock().unwrap();
            q.drain(..).collect()
        };

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
                AlacEvent::ClipboardStore(_, _) => {}
                AlacEvent::ClipboardLoad(_, format) => {
                    self.write_to_pty(format("").into_bytes());
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

        while let Some(event) = self.events.pop_front() {
            match event {
                InternalEvent::Resize(bounds) => {
                    terminal.resize(TermSize {
                        columns: bounds.num_columns(),
                        screen_lines: bounds.num_lines(),
                    });
                    let _ = self.pty_tx.0.send(Msg::Resize(bounds.into()));
                }
                InternalEvent::Scroll(scroll) => {
                    terminal.scroll_display(scroll);
                }
                InternalEvent::SetSelection(sel) => {
                    terminal.selection = sel.map(|(s, _)| s);
                }
                InternalEvent::UpdateSelection(point, side) => {
                    if let Some(mut sel) = terminal.selection.take() {
                        sel.update(point, side);
                        terminal.selection = Some(sel);
                    }
                }
                InternalEvent::Clear => {
                    // 发送清屏转义序列到 PTY（清除屏幕 + 清除滚回）
                    drop(terminal);
                    let _ = self.pty_tx.0.send(Msg::Input(Cow::Borrowed(b"\x1b[2J\x1b[3J\x1b[H")));
                    terminal = term.lock_unfair();
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

        self.last_content = TerminalContent {
            cells,
            mode: content.mode,
            display_offset,
            selection: content.selection,
            selection_text,
            cursor: Some(cursor),
            cursor_char,
            terminal_bounds: self.last_content.terminal_bounds,
            title: self.breadcrumb_text.clone(),
            scrolled_to_top: display_offset == history_size,
            scrolled_to_bottom: display_offset == 0,
        };
    }

    // ─── 公开 API ──────────────────────────────────────────────────────────────

    /// 发送字节到 PTY
    pub fn input(&mut self, data: impl Into<Cow<'static, [u8]>>) {
        self.write_to_pty(data.into());
    }

    /// 处理键盘事件，返回是否已处理
    pub fn try_keystroke(
        &mut self,
        keystroke: &Keystroke,
        option_as_meta: bool,
    ) -> bool {
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
            || old.num_columns() != new_bounds.num_columns()
            || (old.cell_width - new_bounds.cell_width).abs() > 0.5
            || (old.line_height - new_bounds.line_height).abs() > 0.5;

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

    /// 粘贴文本（支持 bracketed paste 模式）
    pub fn paste(&mut self, text: &str) {
        let paste_text = if self
            .last_content
            .mode
            .contains(TermMode::BRACKETED_PASTE)
        {
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

    /// 向上滚动若干行
    pub fn scroll_up_by(&mut self, lines: usize) {
        self.events
            .push_back(InternalEvent::Scroll(AlacScroll::Delta(lines as i32)));
    }

    /// 向下滚动若干行
    pub fn scroll_down_by(&mut self, lines: usize) {
        self.events
            .push_back(InternalEvent::Scroll(AlacScroll::Delta(-(lines as i32))));
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
        self.events
            .push_back(InternalEvent::SetSelection(None));
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
        let _ = self.pty_tx.0.send(Msg::Input(data.into()));
    }
}

impl EventEmitter<Event> for Terminal {}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.pty_tx.0.send(Msg::Shutdown);
    }
}
