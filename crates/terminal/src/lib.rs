mod mappings;
mod pty_info;
mod terminal_hyperlinks;

use alacritty_terminal::{
    Term, event::{Event as aEvent, EventListener, WindowSize}, event_loop::Notifier, grid::{Dimensions, Grid, Row, Scroll as aScroll}, index::{Boundary, Column, Direction as aDirection, Line, Point as aPoint}, selection::{Selection, SelectionRange}, sync::FairMutex, term::{Config, RenderableCursor, TermMode, cell::Cell}, tty::Shell, vi_mode::ViMotion, vte::ansi::CursorShape
};
use gpui::*;
use pty_info::{ProcessIdGetter, PtyProcessInfo};
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    collections::{HashMap, VecDeque},
    fmt::Display,
    ops::{Deref, RangeInclusive},
    path::PathBuf,
    process::ExitStatus,
    sync::Arc,
    time::Instant,
};
use tokio::sync::mpsc::{Receiver, Sender, UnboundedSender};

use crate::terminal_hyperlinks::RegexSearches;

actions!(
    terminal,
    [
        /// Clears the terminal screen.
        Clear,
        /// Copies selected text to the clipboard.
        Copy,
        /// Pastes from the clipboard.
        Paste,
        /// Shows the character palette for special characters.
        ShowCharacterPalette,
        /// Searches for text in the terminal.
        SearchTest,
        /// Scrolls up by one line.
        ScrollLineUp,
        /// Scrolls down by one line.
        ScrollLineDown,
        /// Scrolls up by one page.
        ScrollPageUp,
        /// Scrolls down by one page.
        ScrollPageDown,
        /// Scrolls up by half a page.
        ScrollHalfPageUp,
        /// Scrolls down by half a page.
        ScrollHalfPageDown,
        /// Scrolls to the top of the terminal buffer.
        ScrollToTop,
        /// Scrolls to the bottom of the terminal buffer.
        ScrollToBottom,
        /// Toggles vi mode in the terminal.
        ToggleViMode,
        /// Selects all text in the terminal.
        SelectAll,
    ]
);

const DEBUG_TERMINAL_WIDTH: Pixels = px(500.);
const DEBUG_TERMINAL_HEIGHT: Pixels = px(30.);
const DEBUG_CELL_WIDTH: Pixels = px(5.);
const DEBUG_LINE_HEIGHT: Pixels = px(5.);

///Upward flowing events, for changing the title and such
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    TitleChanged,
    BreadcrumbsChanged,
    CloseTerminal,
    Bell,
    Wakeup,
    BlinkChanged(bool),
    SelectionsChanged,
    NewNavigationTarget(Option<MaybeNavigationTarget>),
    Open(MaybeNavigationTarget),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathLikeTarget {
    /// File system path, absolute or relative, existing or not.
    /// Might have line and column number(s) attached as `file.rs:1:23`
    pub maybe_path: String,
    /// Current working directory of the terminal
    pub terminal_dir: Option<PathBuf>,
}

/// A string inside terminal, potentially useful as a URI that can be opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MaybeNavigationTarget {
    /// HTTP、git 等由“URL_REGEX”正则表达式确定的字符串。
    Url(String),
    /// 文件系统路径，绝对或相对，存在或不存在。
    /// Might have line and column number(s) attached as `file.rs:1:23`
    PathLike(PathLikeTarget),
}

#[derive(Clone)]
enum InternalEvent {
    Resize(TerminalBounds),
    Clear,
    // FocusNextMatch,
    Scroll(aScroll),
    ScrollToAlacPoint(aPoint),
    SetSelection(Option<(Selection, aPoint)>),
    UpdateSelection(Point<Pixels>),
    // Adjusted mouse position, should open
    FindHyperlink(Point<Pixels>, bool),
    // Whether keep selection when copy
    Copy(Option<bool>),
    // Vi mode events
    ToggleViMode,
    ViMotion(ViMotion),
    MoveViCursorToAlacPoint(aPoint),
}
/// Alacritty 的翻译结构可通过其事件循环与我们进行通信
#[derive(Clone)]
pub struct TermListener(pub UnboundedSender<aEvent>);
/// 将事件发送到 Alacritty 事件循环
impl EventListener for TermListener {
    fn send_event(&self, event: aEvent) {
        self.0.send(event).ok();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalBounds {
    pub cell_width: Pixels,
    pub line_height: Pixels,
    pub bounds: Bounds<Pixels>,
}
impl TerminalBounds {
    pub fn new(line_height: Pixels, cell_width: Pixels, bounds: Bounds<Pixels>) -> Self {
        TerminalBounds {
            cell_width,
            line_height,
            bounds,
        }
    }

    pub fn num_lines(&self) -> usize {
        (self.bounds.size.height / self.line_height).floor() as usize
    }

    pub fn num_columns(&self) -> usize {
        (self.bounds.size.width / self.cell_width).floor() as usize
    }

    pub fn height(&self) -> Pixels {
        self.bounds.size.height
    }

    pub fn width(&self) -> Pixels {
        self.bounds.size.width
    }

    pub fn cell_width(&self) -> Pixels {
        self.cell_width
    }

    pub fn line_height(&self) -> Pixels {
        self.line_height
    }
}

impl Default for TerminalBounds {
    fn default() -> Self {
        TerminalBounds::new(
            DEBUG_LINE_HEIGHT,
            DEBUG_CELL_WIDTH,
            Bounds {
                origin: Point::default(),
                size: Size {
                    width: DEBUG_TERMINAL_WIDTH,
                    height: DEBUG_TERMINAL_HEIGHT,
                },
            },
        )
    }
}
impl From<TerminalBounds> for WindowSize {
    fn from(val: TerminalBounds) -> Self {
        WindowSize {
            num_lines: val.num_lines() as u16,
            num_cols: val.num_columns() as u16,
            cell_width: f32::from(val.cell_width()) as u16,
            cell_height: f32::from(val.line_height()) as u16,
        }
    }
}

impl Dimensions for TerminalBounds {
    /// Note: this is supposed to be for the back buffer's length,
    /// but we exclusively use it to resize the terminal, which does not
    /// use this method. We still have to implement it for the trait though,
    /// hence, this comment.
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }

    fn screen_lines(&self) -> usize {
        self.num_lines()
    }

    fn columns(&self) -> usize {
        self.num_columns()
    }
}

enum TerminalType {
    Pty {
        pty_tx: Notifier,
        info: PtyProcessInfo,
    },
    DisplayOnly,
}


#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IndexedCell {
    pub point: aPoint,
    pub cell: Cell,
}

impl Deref for IndexedCell {
    type Target = Cell;

    #[inline]
    fn deref(&self) -> &Cell {
        &self.cell
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct HoveredWord {
    pub word: String,
    pub word_match: RangeInclusive<aPoint>,
    pub id: usize,
}
#[derive(PartialEq, Eq)]
pub enum SelectionPhase {
    Selecting,
    Ended,
}

#[derive(Clone)]
pub struct TerminalContent {
    pub cells: Vec<IndexedCell>,
    pub mode: TermMode,
    pub display_offset: usize,
    pub selection_text: Option<String>,
    pub selection: Option<SelectionRange>,
    pub cursor: RenderableCursor,
    pub cursor_char: char,
    pub terminal_bounds: TerminalBounds,
    pub last_hovered_word: Option<HoveredWord>,
    pub scrolled_to_top: bool,
    pub scrolled_to_bottom: bool,
}

//TODO: 添加终端task任务
// /// A status of the current terminal tab's task.
// #[derive(Debug, Clone, Copy, PartialEq, Eq)]
// pub enum TaskStatus {
//     /// The task had been started, but got cancelled or somehow otherwise it did not
//     /// report its exit code before the terminal event loop was shut down.
//     Unknown,
//     /// The task is started and running currently.
//     Running,
//     /// After the start, the task stopped running and reported its error code back.
//     Completed { success: bool },
// }

// impl TaskStatus {
//     fn register_terminal_exit(&mut self) {
//         if self == &Self::Running {
//             *self = Self::Unknown;
//         }
//     }

//     fn register_task_exit(&mut self, error_code: i32) {
//         *self = TaskStatus::Completed {
//             success: error_code == 0,
//         };
//     }
// }
// #[derive(Debug)]
// pub struct TaskState {
//     pub status: TaskStatus,
//     pub completion_rx: Receiver<Option<ExitStatus>>,
//     pub spawned_task: SpawnInTerminal,
// }


struct CopyTemplate {
    shell: Shell,
    env: HashMap<String, String>,
    cursor_shape: CursorShape,
    // alternate_scroll: AlternateScroll,
    max_scroll_history_lines: Option<usize>,
    path_hyperlink_regexes: Vec<String>,
    path_hyperlink_timeout_ms: u64,
    window_id: u64,
}

pub struct Terminal {
    terminal_type: TerminalType,
    completion_tx: Option<Sender<Option<ExitStatus>>>,
    term: Arc<FairMutex<Term<TermListener>>>,
    term_config: Config,
    events: VecDeque<InternalEvent>,
    /// 这仅用于鼠标模式Cell变化检测
    last_mouse: Option<(aPoint, aDirection)>,
    pub matches: Vec<RangeInclusive<aPoint>>,
    pub last_content: TerminalContent,
    pub selection_head: Option<aPoint>,
    pub breadcrumb_text: String,
    title_override: Option<String>,
    scroll_px: Pixels,
    next_link_id: usize,
    selection_phase: SelectionPhase,
    hyperlink_regex_searches: RegexSearches,
    // task: Option<TaskState>,
    vi_mode_enabled: bool,
    is_remote_terminal: bool,
    last_mouse_move_time: Instant,
    last_hyperlink_search_position: Option<Point<Pixels>>,
    #[cfg(windows)]
    shell_program: Option<String>,
    template: CopyTemplate,
    activation_script: Vec<String>,
    child_exited: Option<ExitStatus>,
    event_loop_task: Task<Result<(), anyhow::Error>>,
}
