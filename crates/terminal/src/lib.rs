mod pty_info;
mod utils;

pub use pty_info::{ProcessInfo, PtyProcessInfo};

use crate::utils::find_in_env;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier, State};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::tty;
use std::borrow::Cow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

struct TermSize {
    columns: usize,
    screen_lines: usize,
    history_size: usize,
}

impl TermSize {
    fn new(columns: usize, screen_lines: usize) -> Self {
        Self {
            columns,
            screen_lines,
            history_size: 10000, // 10000 行历史缓冲区
        }
    }
}

impl Dimensions for TermSize {
    fn columns(&self) -> usize {
        self.columns
    }
    fn screen_lines(&self) -> usize {
        self.screen_lines
    }
    fn total_lines(&self) -> usize {
        self.screen_lines + self.history_size
    }
}
#[derive(Clone)]
struct TermProxy {
    dirty: Arc<AtomicBool>,
}

impl TermProxy {
    fn new(dirty: Arc<AtomicBool>) -> Self {
        Self { dirty }
    }
}

impl EventListener for TermProxy {
    fn send_event(&self, _event: Event) {
        // 标记终端需要重新渲染
        self.dirty.store(true, Ordering::Relaxed);
    }
}

pub struct Terminal {
    term: Arc<FairMutex<Term<TermProxy>>>,
    notifier: Notifier,
    _io_thread: JoinHandle<(EventLoop<tty::Pty, TermProxy>, State)>,
    dirty: Arc<AtomicBool>,
}

impl Terminal {
    pub fn new(config: &Config) -> Self {
        let shell_path;
        if cfg!(target_os = "windows") {
            // 优先使用 PowerShell 7
            shell_path = find_in_env("pwsh.exe");
        } else {
            shell_path = None;
        }
        let shell = shell_path.map(|path| tty::Shell::new(path, vec![]));
        let pty_config = tty::Options {
            shell,
            working_directory: None,
            drain_on_exit: false,
            env: std::collections::HashMap::new(),
            #[cfg(target_os = "windows")]
            escape_args: true,
        };
        let window_size = WindowSize {
            num_lines: 24,
            num_cols: 80,
            cell_width: 1,
            cell_height: 1,
        };

        let pty = tty::new(&pty_config, window_size, 0).expect("创建 PTY 失败");
        let size = TermSize::new(80, 24);

        let dirty = Arc::new(AtomicBool::new(true));
        let proxy = TermProxy::new(dirty.clone());
        let term = Term::new(config.clone(), &size, proxy.clone());
        let term = Arc::new(FairMutex::new(term));

        let event_loop = EventLoop::new(term.clone(), proxy, pty, false, false)
            .expect("EventLoop 初始化失败");

        let notifier = Notifier(event_loop.channel());
        let io_thread = event_loop.spawn();

        Self {
            term,
            notifier,
            _io_thread: io_thread,
            dirty,
        }
    }

    /// 写入数据到终端
    pub fn write(&self, data: &[u8]) {
        let _ = self.notifier.0.send(Msg::Input(Cow::Owned(data.to_vec())));
    }

    /// 获取终端内容用于渲染（支持滚动偏移）
    /// scroll_offset: 0 表示最底部（当前内容），大于 0 表示向上滚动的行数
    pub fn get_content_with_scroll(&self, scroll_offset: usize) -> Vec<String> {
        let term = self.term.lock();
        let grid = term.grid();
        let mut lines = Vec::new();

        let screen_lines = grid.screen_lines();
        let total_lines = grid.total_lines();
        let history_size = total_lines - screen_lines;

        // 限制滚动偏移量不超过历史缓冲区大小
        let offset = scroll_offset.min(history_size);

        // 计算起始行（从历史缓冲区或屏幕区域）
        for i in 0..screen_lines {
            let line_index = i as i32 - offset as i32;
            let line = &grid[Line(line_index)];
            let mut line_str = String::new();
            for j in 0..grid.columns() {
                let cell = &line[Column(j)];
                let c = if cell.c == '\0' { ' ' } else { cell.c };
                line_str.push(c);
            }
            lines.push(line_str);
        }

        lines
    }

    /// 获取终端内容用于渲染
    pub fn get_content(&self) -> Vec<String> {
        self.get_content_with_scroll(0)
    }

    /// 获取历史缓冲区大小
    pub fn history_size(&self) -> usize {
        let term = self.term.lock();
        let grid = term.grid();
        grid.total_lines() - grid.screen_lines()
    }

    /// 获取光标位置 (column, line)
    pub fn cursor_position(&self) -> (usize, usize) {
        let term = self.term.lock();
        let cursor = term.grid().cursor.point;
        (cursor.column.0, cursor.line.0 as usize)
    }

    /// 调整终端大小
    pub fn resize(&self, cols: usize, lines: usize) {
        let size = TermSize::new(cols, lines);
        let mut term = self.term.lock();
        term.resize(size);

        // 同步更新 PTY 窗口大小
        let window_size = WindowSize {
            num_lines: lines as u16,
            num_cols: cols as u16,
            cell_width: 1,
            cell_height: 1,
        };
        let _ = self.notifier.0.send(Msg::Resize(window_size));
    }

    /// 获取终端尺寸 (columns, lines)
    pub fn size(&self) -> (usize, usize) {
        let term = self.term.lock();
        let grid = term.grid();
        (grid.columns(), grid.screen_lines())
    }

    /// 检查终端是否需要重新渲染
    pub fn needs_render(&self) -> bool {
        self.dirty.swap(false, Ordering::Relaxed)
    }
}
