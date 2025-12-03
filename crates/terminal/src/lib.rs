mod mappings;
mod pty_info;
mod terminal_hyperlinks;
mod terminal_settings;
use mappings::mouse::{
    alt_scroll, grid_point, grid_point_and_side, mouse_button_report, mouse_moved_report,
    scroll_report,
};
use crate::terminal_settings::TerminalSettings;
use alacritty_terminal::{
    Term, event::{Event as aEvent, EventListener, Notify, WindowSize}, event_loop::{Msg, Notifier}, grid::{Dimensions, Grid, Row, Scroll as aScroll}, index::{Boundary, Column, Direction as aDirection, Line, Point as aPoint}, selection::{Selection, SelectionRange, SelectionType}, sync::FairMutex, term::{Config, RenderableCursor, TermMode, cell::{Cell, Flags}, search::{Match, RegexIter, RegexSearch}}, tty::Shell, vi_mode::{ViModeCursor, ViMotion}, vte::ansi::{ClearMode, CursorShape,Handler}
};
use gpui::*;
use gpui_component::{ActiveTheme,Theme};
use log::trace;
use pty_info::{ProcessIdGetter, PtyProcessInfo};
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow, cmp::{self, min}, collections::{HashMap, VecDeque}, fmt::Display, ops::{Deref, RangeInclusive}, path::PathBuf, process::ExitStatus, sync::Arc, time::Instant
};
use tokio::sync::mpsc::{Receiver, Sender, UnboundedReceiver, UnboundedSender};
use crate::mappings::{colors::to_alac_rgb, keys::to_esc_str};
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
    ScrollToaPoint(aPoint),
    SetSelection(Option<(Selection, aPoint)>),
    UpdateSelection(Point<Pixels>),
    // Adjusted mouse position, should open
    FindHyperlink(Point<Pixels>, bool),
    // Whether keep selection when copy
    Copy(Option<bool>),
    // Vi mode events
    ToggleViMode,
    ViMotion(ViMotion),
    MoveViCursorToaPoint(aPoint),
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
pub struct TerminalBuilder {
    terminal: Terminal,
    events_rx: UnboundedReceiver<aEvent>,
}

impl TerminalBuilder {
    // pub fn new(
    //     working_directory: Option<PathBuf>,
    //     // task: Option<TaskState>,
    //     shell: Shell,
    //     mut env: HashMap<String, String>,
    //     cursor_shape: CursorShape,
    //     // alternate_scroll: AlternateScroll,
    //     max_scroll_history_lines: Option<usize>,
    //     path_hyperlink_regexes: Vec<String>,
    //     path_hyperlink_timeout_ms: u64,
    //     is_remote_terminal: bool,
    //     window_id: u64,
    //     completion_tx: Option<Sender<Option<ExitStatus>>>,
    //     cx: &App,
    //     activation_script: Vec<String>,
    // ) -> Task<Result<TerminalBuilder>> {
    //     let version = release_channel::AppVersion::global(cx);
    //     let fut = async move {
    //         // If the parent environment doesn't have a locale set
    //         // (As is the case when launched from a .app on MacOS),
    //         // and the Project doesn't have a locale set, then
    //         // set a fallback for our child environment to use.
    //         if std::env::var("LANG").is_err() {
    //             env.entry("LANG".to_string())
    //                 .or_insert_with(|| "en_US.UTF-8".to_string());
    //         }

    //         env.insert("ZED_TERM".to_string(), "true".to_string());
    //         env.insert("TERM_PROGRAM".to_string(), "zed".to_string());
    //         env.insert("TERM".to_string(), "xterm-256color".to_string());
    //         env.insert("COLORTERM".to_string(), "truecolor".to_string());
    //         env.insert("TERM_PROGRAM_VERSION".to_string(), version.to_string());

    //         #[derive(Default)]
    //         struct ShellParams {
    //             program: String,
    //             args: Option<Vec<String>>,
    //             title_override: Option<String>,
    //         }

    //         impl ShellParams {
    //             fn new(
    //                 program: String,
    //                 args: Option<Vec<String>>,
    //                 title_override: Option<String>,
    //             ) -> Self {
    //                 log::debug!("Using {program} as shell");
    //                 Self {
    //                     program,
    //                     args,
    //                     title_override,
    //                 }
    //             }
    //         }

    //         let shell_params = match shell.clone() {
    //             Shell::System => {
    //                 if cfg!(windows) {
    //                     Some(ShellParams::new(
    //                         util::shell::get_windows_system_shell(),
    //                         None,
    //                         None,
    //                     ))
    //                 } else {
    //                     None
    //                 }
    //             }
    //             Shell::Program(program) => Some(ShellParams::new(program, None, None)),
    //             Shell::WithArguments {
    //                 program,
    //                 args,
    //                 title_override,
    //             } => Some(ShellParams::new(program, Some(args), title_override)),
    //         };
    //         let terminal_title_override =
    //             shell_params.as_ref().and_then(|e| e.title_override.clone());

    //         #[cfg(windows)]
    //         let shell_program = shell_params.as_ref().map(|params| {
    //             use util::ResultExt;

    //             Self::resolve_path(&params.program)
    //                 .log_err()
    //                 .unwrap_or(params.program.clone())
    //         });

    //         // Note: when remoting, this shell_kind will scrutinize `ssh` or
    //         // `wsl.exe` as a shell and fall back to posix or powershell based on
    //         // the compilation target. This is fine right now due to the restricted
    //         // way we use the return value, but would become incorrect if we
    //         // supported remoting into windows.
    //         let shell_kind = shell.shell_kind(cfg!(windows));

    //         let pty_options = {
    //             let alac_shell = shell_params.as_ref().map(|params| {
    //                 alacritty_terminal::tty::Shell::new(
    //                     params.program.clone(),
    //                     params.args.clone().unwrap_or_default(),
    //                 )
    //             });

    //             alacritty_terminal::tty::Options {
    //                 shell: alac_shell,
    //                 working_directory: working_directory.clone(),
    //                 drain_on_exit: true,
    //                 env: env.clone().into_iter().collect(),
    //                 #[cfg(windows)]
    //                 escape_args: shell_kind.tty_escape_args(),
    //             }
    //         };

    //         let default_cursor_style = AlacCursorStyle::from(cursor_shape);
    //         let scrolling_history = if task.is_some() {
    //             // Tasks like `cargo build --all` may produce a lot of output, ergo allow maximum scrolling.
    //             // After the task finishes, we do not allow appending to that terminal, so small tasks output should not
    //             // cause excessive memory usage over time.
    //             MAX_SCROLL_HISTORY_LINES
    //         } else {
    //             max_scroll_history_lines
    //                 .unwrap_or(DEFAULT_SCROLL_HISTORY_LINES)
    //                 .min(MAX_SCROLL_HISTORY_LINES)
    //         };
    //         let config = Config {
    //             scrolling_history,
    //             default_cursor_style,
    //             ..Config::default()
    //         };

    //         //Setup the pty...
    //         let pty = match tty::new(&pty_options, TerminalBounds::default().into(), window_id) {
    //             Ok(pty) => pty,
    //             Err(error) => {
    //                 bail!(TerminalError {
    //                     directory: working_directory,
    //                     program: shell_params.as_ref().map(|params| params.program.clone()),
    //                     args: shell_params.as_ref().and_then(|params| params.args.clone()),
    //                     title_override: terminal_title_override,
    //                     source: error,
    //                 });
    //             }
    //         };

    //         //Spawn a task so the Alacritty EventLoop can communicate with us
    //         //TODO: Remove with a bounded sender which can be dispatched on &self
    //         let (events_tx, events_rx) = unbounded();
    //         //Set up the terminal...
    //         let mut term = Term::new(
    //             config.clone(),
    //             &TerminalBounds::default(),
    //             ZedListener(events_tx.clone()),
    //         );

    //         //Alacritty defaults to alternate scrolling being on, so we just need to turn it off.
    //         if let AlternateScroll::Off = alternate_scroll {
    //             term.unset_private_mode(PrivateMode::Named(NamedPrivateMode::AlternateScroll));
    //         }

    //         let term = Arc::new(FairMutex::new(term));

    //         let pty_info = PtyProcessInfo::new(&pty);

    //         //And connect them together
    //         let event_loop = EventLoop::new(
    //             term.clone(),
    //             ZedListener(events_tx),
    //             pty,
    //             pty_options.drain_on_exit,
    //             false,
    //         )
    //         .context("failed to create event loop")?;

    //         let pty_tx = event_loop.channel();
    //         let _io_thread = event_loop.spawn(); // DANGER

    //         let no_task = task.is_none();
    //         let terminal = Terminal {
    //             task,
    //             terminal_type: TerminalType::Pty {
    //                 pty_tx: Notifier(pty_tx),
    //                 info: pty_info,
    //             },
    //             completion_tx,
    //             term,
    //             term_config: config,
    //             title_override: terminal_title_override,
    //             events: VecDeque::with_capacity(10), //Should never get this high.
    //             last_content: Default::default(),
    //             last_mouse: None,
    //             matches: Vec::new(),
    //             selection_head: None,
    //             breadcrumb_text: String::new(),
    //             scroll_px: px(0.),
    //             next_link_id: 0,
    //             selection_phase: SelectionPhase::Ended,
    //             hyperlink_regex_searches: RegexSearches::new(
    //                 &path_hyperlink_regexes,
    //                 path_hyperlink_timeout_ms,
    //             ),
    //             vi_mode_enabled: false,
    //             is_remote_terminal,
    //             last_mouse_move_time: Instant::now(),
    //             last_hyperlink_search_position: None,
    //             #[cfg(windows)]
    //             shell_program,
    //             activation_script: activation_script.clone(),
    //             template: CopyTemplate {
    //                 shell,
    //                 env,
    //                 cursor_shape,
    //                 alternate_scroll,
    //                 max_scroll_history_lines,
    //                 path_hyperlink_regexes,
    //                 path_hyperlink_timeout_ms,
    //                 window_id,
    //             },
    //             child_exited: None,
    //             event_loop_task: Task::ready(Ok(())),
    //         };

    //         if !activation_script.is_empty() && no_task {
    //             for activation_script in activation_script {
    //                 terminal.write_to_pty(activation_script.into_bytes());
    //                 // Simulate enter key press
    //                 // NOTE(PowerShell): using `\r\n` will put PowerShell in a continuation mode (infamous >> character)
    //                 // and generally mess up the rendering.
    //                 terminal.write_to_pty(b"\x0d");
    //             }
    //             // In order to clear the screen at this point, we have two options:
    //             // 1. We can send a shell-specific command such as "clear" or "cls"
    //             // 2. We can "echo" a marker message that we will then catch when handling a Wakeup event
    //             //    and clear the screen using `terminal.clear()` method
    //             // We cannot issue a `terminal.clear()` command at this point as alacritty is evented
    //             // and while we have sent the activation script to the pty, it will be executed asynchronously.
    //             // Therefore, we somehow need to wait for the activation script to finish executing before we
    //             // can proceed with clearing the screen.
    //             terminal.write_to_pty(shell_kind.clear_screen_command().as_bytes());
    //             // Simulate enter key press
    //             terminal.write_to_pty(b"\x0d");
    //         }

    //         Ok(TerminalBuilder {
    //             terminal,
    //             events_rx,
    //         })
    //     };
    //     // the thread we spawn things on has an effect on signal handling
    //     if !cfg!(target_os = "windows") {
    //         cx.spawn(async move |_| fut.await)
    //     } else {
    //         cx.background_spawn(fut)
    //     }
    // }

    // pub fn subscribe(mut self, cx: &Context<Terminal>) -> Terminal {
    //     //Event loop
    //     // self.terminal.event_loop_task = cx.spawn(async move |terminal, cx| {
    //     //     while let Some(event) = self.events_rx.next().await {
    //     //         terminal.update(cx, |terminal, cx| {
    //     //             //Process the first event immediately for lowered latency
    //     //             terminal.process_event(event, cx);
    //     //         })?;

    //     //         'outer: loop {
    //     //             let mut events = Vec::new();

    //     //             #[cfg(any(test, feature = "test-support"))]
    //     //             let mut timer = cx.background_executor().simulate_random_delay().fuse();
    //     //             #[cfg(not(any(test, feature = "test-support")))]
    //     //             let mut timer = cx
    //     //                 .background_executor()
    //     //                 .timer(std::time::Duration::from_millis(4))
    //     //                 .fuse();

    //     //             let mut wakeup = false;
    //     //             loop {
    //     //                 futures::select_biased! {
    //     //                     _ = timer => break,
    //     //                     event = self.events_rx.next() => {
    //     //                         if let Some(event) = event {
    //     //                             if matches!(event, aEvent::Wakeup) {
    //     //                                 wakeup = true;
    //     //                             } else {
    //     //                                 events.push(event);
    //     //                             }

    //     //                             if events.len() > 100 {
    //     //                                 break;
    //     //                             }
    //     //                         } else {
    //     //                             break;
    //     //                         }
    //     //                     },
    //     //                 }
    //     //             }

    //     //             if events.is_empty() && !wakeup {
    //     //                 smol::future::yield_now().await;
    //     //                 break 'outer;
    //     //             }

    //     //             terminal.update(cx, |this, cx| {
    //     //                 if wakeup {
    //     //                     this.process_event(aEvent::Wakeup, cx);
    //     //                 }

    //     //                 for event in events {
    //     //                     this.process_event(event, cx);
    //     //                 }
    //     //             })?;
    //     //             smol::future::yield_now().await;
    //     //         }
    //     //     }
    //     //     anyhow::Ok(())
    //     // });
    //     self.terminal
    // }

    // #[cfg(windows)]
    // fn resolve_path(path: &str) -> Result<String> {
    //     use windows::Win32::Storage::FileSystem::SearchPathW;
    //     use windows::core::HSTRING;

    //     let path = if path.starts_with(r"\\?\") || !path.contains(&['/', '\\']) {
    //         path.to_string()
    //     } else {
    //         r"\\?\".to_string() + path
    //     };

    //     let required_length = unsafe { SearchPathW(None, &HSTRING::from(&path), None, None, None) };
    //     let mut buf = vec![0u16; required_length as usize];
    //     let size = unsafe { SearchPathW(None, &HSTRING::from(&path), None, Some(&mut buf), None) };

        // Ok(String::from_utf16(&buf[..size as usize])?)

    // }
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
impl Drop for Terminal {
    fn drop(&mut self) {
        if let TerminalType::Pty { pty_tx, info } = &mut self.terminal_type {
            info.kill_child_process();
            pty_tx.0.send(Msg::Shutdown).ok();
        }
    }
}

impl EventEmitter<Event> for Terminal {}
impl Terminal {
    // fn process_event(&mut self, event: aEvent, cx: &mut Context<Self>) {
    //     match event {
    //         aEvent::Title(title) => {
    //             // 忽略默认 shell 程序标题更改，因为 Windows 总是发送这些事件
    //             // 它最终会在面包屑中显示 shell 可执行路径
    //             #[cfg(windows)]
    //             {
    //                 if self
    //                     .shell_program
    //                     .as_ref()
    //                     .map(|e| *e == title)
    //                     .unwrap_or(false)
    //                 {
    //                     return;
    //                 }
    //             }

    //             self.breadcrumb_text = title;
    //             cx.emit(Event::BreadcrumbsChanged);
    //         }
    //         aEvent::ResetTitle => {
    //             self.breadcrumb_text = String::new();
    //             cx.emit(Event::BreadcrumbsChanged);
    //         }
    //         aEvent::ClipboardStore(_, data) => {
    //             cx.write_to_clipboard(ClipboardItem::new_string(data))
    //         }
    //         aEvent::ClipboardLoad(_, format) => {
    //             self.write_to_pty(
    //                 match &cx.read_from_clipboard().and_then(|item| item.text()) {
    //                     // The terminal only supports pasting strings, not images.
    //                     Some(text) => format(text),
    //                     _ => format(""),
    //                 }
    //                 .into_bytes(),
    //             )
    //         }
    //         aEvent::PtyWrite(out) => self.write_to_pty(out.into_bytes()),
    //         aEvent::TextAreaSizeRequest(format) => {
    //             self.write_to_pty(format(self.last_content.terminal_bounds.into()).into_bytes())
    //         }
    //         aEvent::CursorBlinkingChange => {
    //             let terminal = self.term.lock();
    //             let blinking = terminal.cursor_style().blinking;
    //             cx.emit(Event::BlinkChanged(blinking));
    //         }
    //         aEvent::Bell => {
    //             cx.emit(Event::Bell);
    //         }
    //         aEvent::Exit => self.register_task_finished(None, cx),
    //         aEvent::MouseCursorDirty => {
    //             //NOOP, Handled in render
    //         }
    //         aEvent::Wakeup => {
    //             cx.emit(Event::Wakeup);

    //             if let TerminalType::Pty { info, .. } = &mut self.terminal_type {
    //                 if info.has_changed() {
    //                     cx.emit(Event::TitleChanged);
    //                 }
    //             }
    //         }
    //         aEvent::ColorRequest(index, format) => {
    //             // 重要的是，在这里处理颜色请求以保持相对顺序
    //             // 与其他 PTY 写入。否则应用程序可能会出现乱序
    //             // 对请求的响应。例如：应用程序发送“OSC 11”； ？ ST`
    //             // （颜色请求）后跟“CSI c”（请求设备属性）将收到
    //             // 首先是对“CSI c”的响应。
    //             // 我们可以将颜色存储在“self.last_content”中，而不是锁定。但后来
    //             // 如果立即执行“设置颜色”序列，我们可能会返回过时的值
    //             // 随后是颜色请求序列。
    //             let color = self.term.lock().colors()[index]
    //                 .unwrap_or_else(|| to_alac_rgb(get_color_at_index(index, cx.theme())));
    //             self.write_to_pty(format(color).into_bytes());
    //         }
    //         aEvent::ChildExit(error_code) => {
    //             self.register_task_finished(Some(error_code), cx);
    //         }
    //     }
    // }

    pub fn selection_started(&self) -> bool {
        self.selection_phase == SelectionPhase::Selecting
    }

    // fn process_terminal_event(
    //     &mut self,
    //     event: &InternalEvent,
    //     term: &mut Term<TermListener>,
    //     window: &mut Window,
    //     cx: &mut Context<Self>,
    // ) {
    //     match event {
    //         &InternalEvent::Resize(mut new_bounds) => {
    //             trace!("Resizing: new_bounds={new_bounds:?}");
    //             new_bounds.bounds.size.height =
    //                 cmp::max(new_bounds.line_height, new_bounds.height());
    //             new_bounds.bounds.size.width = cmp::max(new_bounds.cell_width, new_bounds.width());

    //             self.last_content.terminal_bounds = new_bounds;

    //             if let TerminalType::Pty { pty_tx, .. } = &self.terminal_type {
    //                 pty_tx.0.send(Msg::Resize(new_bounds.into())).ok();
    //             }

    //             term.resize(new_bounds);
    //         }
    //         InternalEvent::Clear => {
    //             trace!("Clearing");
    //             // Clear back buffer
    //             term.clear_screen(ClearMode::Saved);

    //             let cursor = term.grid().cursor.point;

    //             // Clear the lines above
    //             term.grid_mut().reset_region(..cursor.line);

    //             // Copy the current line up
    //             let line = term.grid()[cursor.line][..Column(term.grid().columns())]
    //                 .iter()
    //                 .cloned()
    //                 .enumerate()
    //                 .collect::<Vec<(usize, Cell)>>();

    //             for (i, cell) in line {
    //                 term.grid_mut()[Line(0)][Column(i)] = cell;
    //             }

    //             // Reset the cursor
    //             term.grid_mut().cursor.point =
    //                 aPoint::new(Line(0), term.grid_mut().cursor.point.column);
    //             let new_cursor = term.grid().cursor.point;

    //             // Clear the lines below the new cursor
    //             if (new_cursor.line.0 as usize) < term.screen_lines() - 1 {
    //                 term.grid_mut().reset_region((new_cursor.line + 1)..);
    //             }

    //             cx.emit(Event::Wakeup);
    //         }
    //         InternalEvent::Scroll(scroll) => {
    //             trace!("Scrolling: scroll={scroll:?}");
    //             term.scroll_display(*scroll);
    //             self.refresh_hovered_word(window);

    //             if self.vi_mode_enabled {
    //                 match *scroll {
    //                     aScroll::Delta(delta) => {
    //                         term.vi_mode_cursor = term.vi_mode_cursor.scroll(term, delta);
    //                     }
    //                     aScroll::PageUp => {
    //                         let lines = term.screen_lines() as i32;
    //                         term.vi_mode_cursor = term.vi_mode_cursor.scroll(term, lines);
    //                     }
    //                     aScroll::PageDown => {
    //                         let lines = -(term.screen_lines() as i32);
    //                         term.vi_mode_cursor = term.vi_mode_cursor.scroll(term, lines);
    //                     }
    //                     aScroll::Top => {
    //                         let point = aPoint::new(term.topmost_line(), Column(0));
    //                         term.vi_mode_cursor = ViModeCursor::new(point);
    //                     }
    //                     aScroll::Bottom => {
    //                         let point = aPoint::new(term.bottommost_line(), Column(0));
    //                         term.vi_mode_cursor = ViModeCursor::new(point);
    //                     }
    //                 }
    //                 if let Some(mut selection) = term.selection.take() {
    //                     let point = term.vi_mode_cursor.point;
    //                     selection.update(point, aDirection::Right);
    //                     term.selection = Some(selection);

    //                     #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    //                     if let Some(selection_text) = term.selection_to_string() {
    //                         cx.write_to_primary(ClipboardItem::new_string(selection_text));
    //                     }

    //                     self.selection_head = Some(point);
    //                     cx.emit(Event::SelectionsChanged)
    //                 }
    //             }
    //         }
    //         InternalEvent::SetSelection(selection) => {
    //             trace!("Setting selection: selection={selection:?}");
    //             term.selection = selection.as_ref().map(|(sel, _)| sel.clone());

    //             #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    //             if let Some(selection_text) = term.selection_to_string() {
    //                 cx.write_to_primary(ClipboardItem::new_string(selection_text));
    //             }

    //             if let Some((_, head)) = selection {
    //                 self.selection_head = Some(*head);
    //             }
    //             cx.emit(Event::SelectionsChanged)
    //         }
    //         InternalEvent::UpdateSelection(position) => {
    //             trace!("Updating selection: position={position:?}");
    //             if let Some(mut selection) = term.selection.take() {
    //                 let (point, side) = grid_point_and_side(
    //                     *position,
    //                     self.last_content.terminal_bounds,
    //                     term.grid().display_offset(),
    //                 );

    //                 selection.update(point, side);
    //                 term.selection = Some(selection);

    //                 #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    //                 if let Some(selection_text) = term.selection_to_string() {
    //                     cx.write_to_primary(ClipboardItem::new_string(selection_text));
    //                 }

    //                 self.selection_head = Some(point);
    //                 cx.emit(Event::SelectionsChanged)
    //             }
    //         }

    //         InternalEvent::Copy(keep_selection) => {
    //             trace!("Copying selection: keep_selection={keep_selection:?}");
    //             if let Some(txt) = term.selection_to_string() {
    //                 cx.write_to_clipboard(ClipboardItem::new_string(txt));
    //                 if !keep_selection.unwrap_or_else(|| {
    //                     let settings = TerminalSettings::get_global(cx);
    //                     settings.keep_selection_on_copy
    //                 }) {
    //                     self.events.push_back(InternalEvent::SetSelection(None));
    //                 }
    //             }
    //         }
    //         InternalEvent::ScrollToaPoint(point) => {
    //             trace!("Scrolling to point: point={point:?}");
    //             term.scroll_to_point(*point);
    //             self.refresh_hovered_word(window);
    //         }
    //         InternalEvent::MoveViCursorToaPoint(point) => {
    //             trace!("Move vi cursor to point: point={point:?}");
    //             term.vi_goto_point(*point);
    //             self.refresh_hovered_word(window);
    //         }
    //         InternalEvent::ToggleViMode => {
    //             trace!("Toggling vi mode");
    //             self.vi_mode_enabled = !self.vi_mode_enabled;
    //             term.toggle_vi_mode();
    //         }
    //         InternalEvent::ViMotion(motion) => {
    //             trace!("Performing vi motion: motion={motion:?}");
    //             term.vi_motion(*motion);
    //         }
    //         InternalEvent::FindHyperlink(position, open) => {
    //             trace!("Finding hyperlink at position: position={position:?}, open={open:?}");
    //             let prev_hovered_word = self.last_content.last_hovered_word.take();

    //             let point = grid_point(
    //                 *position,
    //                 self.last_content.terminal_bounds,
    //                 term.grid().display_offset(),
    //             )
    //             .grid_clamp(term, Boundary::Grid);

    //             match terminal_hyperlinks::find_from_grid_point(
    //                 term,
    //                 point,
    //                 &mut self.hyperlink_regex_searches,
    //             ) {
    //                 Some((maybe_url_or_path, is_url, url_match)) => {
    //                     let target = if is_url {
    //                         // Treat "file://" URLs like file paths to ensure
    //                         // that line numbers at the end of the path are
    //                         // handled correctly.
    //                         // file://{path} should be urldecoded, returning a urldecoded {path}
    //                         if let Some(path) = maybe_url_or_path.strip_prefix("file://") {
    //                             let decoded_path = urlencoding::decode(path)
    //                                 .map(|decoded| decoded.into_owned())
    //                                 .unwrap_or(path.to_owned());

    //                             MaybeNavigationTarget::PathLike(PathLikeTarget {
    //                                 maybe_path: decoded_path,
    //                                 terminal_dir: self.working_directory(),
    //                             })
    //                         } else {
    //                             MaybeNavigationTarget::Url(maybe_url_or_path.clone())
    //                         }
    //                     } else {
    //                         MaybeNavigationTarget::PathLike(PathLikeTarget {
    //                             maybe_path: maybe_url_or_path.clone(),
    //                             terminal_dir: self.working_directory(),
    //                         })
    //                     };
    //                     if *open {
    //                         cx.emit(Event::Open(target));
    //                     } else {
    //                         self.update_selected_word(
    //                             prev_hovered_word,
    //                             url_match,
    //                             maybe_url_or_path,
    //                             target,
    //                             cx,
    //                         );
    //                     }
    //                 }
    //                 None => {
    //                     cx.emit(Event::NewNavigationTarget(None));
    //                 }
    //             }
    //         }
    //     }
    // }

    fn update_selected_word(
        &mut self,
        prev_word: Option<HoveredWord>,
        word_match: RangeInclusive<aPoint>,
        word: String,
        navigation_target: MaybeNavigationTarget,
        cx: &mut Context<Self>,
    ) {
        if let Some(prev_word) = prev_word
            && prev_word.word == word
            && prev_word.word_match == word_match
        {
            self.last_content.last_hovered_word = Some(HoveredWord {
                word,
                word_match,
                id: prev_word.id,
            });
            return;
        }

        self.last_content.last_hovered_word = Some(HoveredWord {
            word,
            word_match,
            id: self.next_link_id(),
        });
        cx.emit(Event::NewNavigationTarget(Some(navigation_target)));
        cx.notify()
    }

    fn next_link_id(&mut self) -> usize {
        let res = self.next_link_id;
        self.next_link_id = self.next_link_id.wrapping_add(1);
        res
    }

    pub fn last_content(&self) -> &TerminalContent {
        &self.last_content
    }

    // pub fn set_cursor_shape(&mut self, cursor_shape: CursorShape) {
    //     self.term_config.default_cursor_style = cursor_shape.into();
    //     self.term.lock().set_options(self.term_config.clone());
    // }

    pub fn write_output(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        // Inject bytes directly into the terminal emulator and refresh the UI.
        // This bypasses the PTY/event loop for display-only terminals.
        //
        // We first convert LF to CRLF, to get the expected line wrapping in Alacritty.
        // When output comes from piped commands (not a PTY) such as codex-acp, and that
        // output only contains LF (\n) without a CR (\r) after it, such as the output
        // of the `ls` command when running outside a PTY, Alacritty moves the cursor
        // cursor down a line but does not move it back to the initial column. This makes
        // the rendered output look ridiculous. To prevent this, we insert a CR (\r) before
        // each LF that didn't already have one. (Alacritty doesn't have a setting for this.)
        let mut converted = Vec::with_capacity(bytes.len());
        let mut prev_byte = 0u8;
        for &byte in bytes {
            if byte == b'\n' && prev_byte != b'\r' {
                converted.push(b'\r');
            }
            converted.push(byte);
            prev_byte = byte;
        }

        let mut processor = alacritty_terminal::vte::ansi::Processor::<
            alacritty_terminal::vte::ansi::StdSyncHandler,
        >::new();
        {
            let mut term = self.term.lock();
            processor.advance(&mut *term, &converted);
        }
        cx.emit(Event::Wakeup);
    }

    pub fn total_lines(&self) -> usize {
        self.term.lock_unfair().total_lines()
    }

    pub fn viewport_lines(&self) -> usize {
        self.term.lock_unfair().screen_lines()
    }

    //To test:
    //- Activate match on terminal (scrolling and selection)
    //- Editor search snapping behavior

    pub fn activate_match(&mut self, index: usize) {
        if let Some(search_match) = self.matches.get(index).cloned() {
            self.set_selection(Some((make_selection(&search_match), *search_match.end())));
            if self.vi_mode_enabled {
                self.events
                    .push_back(InternalEvent::MoveViCursorToaPoint(*search_match.end()));
            } else {
                self.events
                    .push_back(InternalEvent::ScrollToaPoint(*search_match.start()));
            }
        }
    }

    pub fn select_matches(&mut self, matches: &[RangeInclusive<aPoint>]) {
        let matches_to_select = self
            .matches
            .iter()
            .filter(|self_match| matches.contains(self_match))
            .cloned()
            .collect::<Vec<_>>();
        for match_to_select in matches_to_select {
            self.set_selection(Some((
                make_selection(&match_to_select),
                *match_to_select.end(),
            )));
        }
    }

    pub fn select_all(&mut self) {
        let term = self.term.lock();
        let start = aPoint::new(term.topmost_line(), Column(0));
        let end = aPoint::new(term.bottommost_line(), term.last_column());
        drop(term);
        self.set_selection(Some((make_selection(&(start..=end)), end)));
    }

    fn set_selection(&mut self, selection: Option<(Selection, aPoint)>) {
        self.events
            .push_back(InternalEvent::SetSelection(selection));
    }

    pub fn copy(&mut self, keep_selection: Option<bool>) {
        self.events.push_back(InternalEvent::Copy(keep_selection));
    }

    pub fn clear(&mut self) {
        self.events.push_back(InternalEvent::Clear)
    }

    pub fn scroll_line_up(&mut self) {
        self.events
            .push_back(InternalEvent::Scroll(aScroll::Delta(1)));
    }

    pub fn scroll_up_by(&mut self, lines: usize) {
        self.events
            .push_back(InternalEvent::Scroll(aScroll::Delta(lines as i32)));
    }

    pub fn scroll_line_down(&mut self) {
        self.events
            .push_back(InternalEvent::Scroll(aScroll::Delta(-1)));
    }

    pub fn scroll_down_by(&mut self, lines: usize) {
        self.events
            .push_back(InternalEvent::Scroll(aScroll::Delta(-(lines as i32))));
    }

    pub fn scroll_page_up(&mut self) {
        self.events
            .push_back(InternalEvent::Scroll(aScroll::PageUp));
    }

    pub fn scroll_page_down(&mut self) {
        self.events
            .push_back(InternalEvent::Scroll(aScroll::PageDown));
    }

    pub fn scroll_to_top(&mut self) {
        self.events
            .push_back(InternalEvent::Scroll(aScroll::Top));
    }

    pub fn scroll_to_bottom(&mut self) {
        self.events
            .push_back(InternalEvent::Scroll(aScroll::Bottom));
    }

    pub fn scrolled_to_top(&self) -> bool {
        self.last_content.scrolled_to_top
    }

    pub fn scrolled_to_bottom(&self) -> bool {
        self.last_content.scrolled_to_bottom
    }

    ///Resize the terminal and the PTY.
    pub fn set_size(&mut self, new_bounds: TerminalBounds) {
        if self.last_content.terminal_bounds != new_bounds {
            self.events.push_back(InternalEvent::Resize(new_bounds))
        }
    }

    /// Write the Input payload to the PTY, if applicable.
    /// (This is a no-op for display-only terminals.)
    fn write_to_pty(&self, input: impl Into<Cow<'static, [u8]>>) {
        if let TerminalType::Pty { pty_tx, .. } = &self.terminal_type {
            let input = input.into();
            if log::log_enabled!(log::Level::Debug) {
                if let Ok(str) = str::from_utf8(&input) {
                    log::debug!("Writing to PTY: {:?}", str);
                } else {
                    log::debug!("Writing to PTY: {:?}", input);
                }
            }
            pty_tx.notify(input);
        }
    }

    pub fn input(&mut self, input: impl Into<Cow<'static, [u8]>>) {
        self.events
            .push_back(InternalEvent::Scroll(aScroll::Bottom));
        self.events.push_back(InternalEvent::SetSelection(None));

        self.write_to_pty(input);
    }

    pub fn toggle_vi_mode(&mut self) {
        self.events.push_back(InternalEvent::ToggleViMode);
    }

    pub fn vi_motion(&mut self, keystroke: &Keystroke) {
        if !self.vi_mode_enabled {
            return;
        }

        let key: Cow<'_, str> = if keystroke.modifiers.shift {
            Cow::Owned(keystroke.key.to_uppercase())
        } else {
            Cow::Borrowed(keystroke.key.as_str())
        };

        let motion: Option<ViMotion> = match key.as_ref() {
            "h" | "left" => Some(ViMotion::Left),
            "j" | "down" => Some(ViMotion::Down),
            "k" | "up" => Some(ViMotion::Up),
            "l" | "right" => Some(ViMotion::Right),
            "w" => Some(ViMotion::WordRight),
            "b" if !keystroke.modifiers.control => Some(ViMotion::WordLeft),
            "e" => Some(ViMotion::WordRightEnd),
            "%" => Some(ViMotion::Bracket),
            "$" => Some(ViMotion::Last),
            "0" => Some(ViMotion::First),
            "^" => Some(ViMotion::FirstOccupied),
            "H" => Some(ViMotion::High),
            "M" => Some(ViMotion::Middle),
            "L" => Some(ViMotion::Low),
            _ => None,
        };

        if let Some(motion) = motion {
            let cursor = self.last_content.cursor.point;
            let cursor_pos = Point {
                x: cursor.column.0 as f32 * self.last_content.terminal_bounds.cell_width,
                y: cursor.line.0 as f32 * self.last_content.terminal_bounds.line_height,
            };
            self.events
                .push_back(InternalEvent::UpdateSelection(cursor_pos));
            self.events.push_back(InternalEvent::ViMotion(motion));
            return;
        }

        let scroll_motion = match key.as_ref() {
            "g" => Some(aScroll::Top),
            "G" => Some(aScroll::Bottom),
            "b" if keystroke.modifiers.control => Some(aScroll::PageUp),
            "f" if keystroke.modifiers.control => Some(aScroll::PageDown),
            "d" if keystroke.modifiers.control => {
                let amount = self.last_content.terminal_bounds.line_height().to_f64() as i32 / 2;
                Some(aScroll::Delta(-amount))
            }
            "u" if keystroke.modifiers.control => {
                let amount = self.last_content.terminal_bounds.line_height().to_f64() as i32 / 2;
                Some(aScroll::Delta(amount))
            }
            _ => None,
        };

        if let Some(scroll_motion) = scroll_motion {
            self.events.push_back(InternalEvent::Scroll(scroll_motion));
            return;
        }

        match key.as_ref() {
            "v" => {
                let point = self.last_content.cursor.point;
                let selection_type = SelectionType::Simple;
                let side = aDirection::Right;
                let selection = Selection::new(selection_type, point, side);
                self.events
                    .push_back(InternalEvent::SetSelection(Some((selection, point))));
            }

            "escape" => {
                self.events.push_back(InternalEvent::SetSelection(None));
            }

            "y" => {
                self.copy(Some(false));
            }

            "i" => {
                self.scroll_to_bottom();
                self.toggle_vi_mode();
            }
            _ => {}
        }
    }

    pub fn try_keystroke(&mut self, keystroke: &Keystroke, option_as_meta: bool) -> bool {
        if self.vi_mode_enabled {
            self.vi_motion(keystroke);
            return true;
        }

        // Keep default terminal behavior
        let esc = to_esc_str(keystroke, &self.last_content.mode, option_as_meta);
        if let Some(esc) = esc {
            match esc {
                Cow::Borrowed(string) => self.input(string.as_bytes()),
                Cow::Owned(string) => self.input(string.into_bytes()),
            };
            true
        } else {
            false
        }
    }

    pub fn try_modifiers_change(
        &mut self,
        modifiers: &Modifiers,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .last_content
            .terminal_bounds
            .bounds
            .contains(&window.mouse_position())
            && modifiers.secondary()
        {
            self.refresh_hovered_word(window);
        }
        cx.notify();
    }

    ///Paste text into the terminal
    pub fn paste(&mut self, text: &str) {
        let paste_text = if self.last_content.mode.contains(TermMode::BRACKETED_PASTE) {
            format!("{}{}{}", "\x1b[200~", text.replace('\x1b', ""), "\x1b[201~")
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r")
        };

        self.input(paste_text.into_bytes());
    }

    pub fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let term = self.term.clone();
        let mut terminal = term.lock_unfair();
        //Note that the ordering of events matters for event processing
        while let Some(e) = self.events.pop_front() {
            // self.process_terminal_event(&e, &mut terminal, window, cx)
        }

        self.last_content = Self::make_content(&terminal, &self.last_content);
    }

    fn make_content(term: &Term<TermListener>, last_content: &TerminalContent) -> TerminalContent {
        let content = term.renderable_content();

        // Pre-allocate with estimated size to reduce reallocations
        let estimated_size = content.display_iter.size_hint().0;
        let mut cells = Vec::with_capacity(estimated_size);

        cells.extend(content.display_iter.map(|ic| IndexedCell {
            point: ic.point,
            cell: ic.cell.clone(),
        }));

        let selection_text = if content.selection.is_some() {
            term.selection_to_string()
        } else {
            None
        };

        TerminalContent {
            cells,
            mode: content.mode,
            display_offset: content.display_offset,
            selection_text,
            selection: content.selection,
            cursor: content.cursor,
            cursor_char: term.grid()[content.cursor.point].c,
            terminal_bounds: last_content.terminal_bounds,
            last_hovered_word: last_content.last_hovered_word.clone(),
            scrolled_to_top: content.display_offset == term.history_size(),
            scrolled_to_bottom: content.display_offset == 0,
        }
    }

    pub fn get_content(&self) -> String {
        let term = self.term.lock_unfair();
        let start = aPoint::new(term.topmost_line(), Column(0));
        let end = aPoint::new(term.bottommost_line(), term.last_column());
        term.bounds_to_string(start, end)
    }

    pub fn last_n_non_empty_lines(&self, n: usize) -> Vec<String> {
        let term = self.term.clone();
        let terminal = term.lock_unfair();
        let grid = terminal.grid();
        let mut lines = Vec::new();

        let mut current_line = grid.bottommost_line().0;
        let topmost_line = grid.topmost_line().0;

        while current_line >= topmost_line && lines.len() < n {
            let logical_line_start = self.find_logical_line_start(grid, current_line, topmost_line);
            let logical_line = self.construct_logical_line(grid, logical_line_start, current_line);

            if let Some(line) = self.process_line(logical_line) {
                lines.push(line);
            }

            // Move to the line above the start of the current logical line
            current_line = logical_line_start - 1;
        }

        lines.reverse();
        lines
    }

    fn find_logical_line_start(&self, grid: &Grid<Cell>, current: i32, topmost: i32) -> i32 {
        let mut line_start = current;
        while line_start > topmost {
            let prev_line = Line(line_start - 1);
            let last_cell = &grid[prev_line][Column(grid.columns() - 1)];
            if !last_cell.flags.contains(Flags::WRAPLINE) {
                break;
            }
            line_start -= 1;
        }
        line_start
    }

    fn construct_logical_line(&self, grid: &Grid<Cell>, start: i32, end: i32) -> String {
        let mut logical_line = String::new();
        for row in start..=end {
            let grid_row = &grid[Line(row)];
            logical_line.push_str(&row_to_string(grid_row));
        }
        logical_line
    }

    fn process_line(&self, line: String) -> Option<String> {
        let trimmed = line.trim_end().to_string();
        if !trimmed.is_empty() {
            Some(trimmed)
        } else {
            None
        }
    }

    pub fn focus_in(&self) {
        if self.last_content.mode.contains(TermMode::FOCUS_IN_OUT) {
            self.write_to_pty("\x1b[I".as_bytes());
        }
    }

    pub fn focus_out(&mut self) {
        if self.last_content.mode.contains(TermMode::FOCUS_IN_OUT) {
            self.write_to_pty("\x1b[O".as_bytes());
        }
    }

    pub fn mouse_changed(&mut self, point: aPoint, side: aDirection) -> bool {
        match self.last_mouse {
            Some((old_point, old_side)) => {
                if old_point == point && old_side == side {
                    false
                } else {
                    self.last_mouse = Some((point, side));
                    true
                }
            }
            None => {
                self.last_mouse = Some((point, side));
                true
            }
        }
    }

    pub fn mouse_mode(&self, shift: bool) -> bool {
        self.last_content.mode.intersects(TermMode::MOUSE_MODE) && !shift
    }

    pub fn mouse_move(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        let position = e.position - self.last_content.terminal_bounds.bounds.origin;
        if self.mouse_mode(e.modifiers.shift) {
            let (point, side) = grid_point_and_side(
                position,
                self.last_content.terminal_bounds,
                self.last_content.display_offset,
            );

            if self.mouse_changed(point, side)
                && let Some(bytes) =
                    mouse_moved_report(point, e.pressed_button, e.modifiers, self.last_content.mode)
            {
                self.write_to_pty(bytes);
            }
        } else if e.modifiers.secondary() {
            self.word_from_position(e.position);
        }
        cx.notify();
    }

    fn word_from_position(&mut self, position: Point<Pixels>) {
        if self.selection_phase == SelectionPhase::Selecting {
            self.last_content.last_hovered_word = None;
        } else if self.last_content.terminal_bounds.bounds.contains(&position) {
            // Throttle hyperlink searches to avoid excessive processing
            let now = Instant::now();
            let should_search = if let Some(last_pos) = self.last_hyperlink_search_position {
                // Only search if mouse moved significantly or enough time passed
                let distance_moved =
                    ((position.x - last_pos.x).abs() + (position.y - last_pos.y).abs()) > px(5.0);
                let time_elapsed = now.duration_since(self.last_mouse_move_time).as_millis() > 100;
                distance_moved || time_elapsed
            } else {
                true
            };

            if should_search {
                self.last_mouse_move_time = now;
                self.last_hyperlink_search_position = Some(position);
                self.events.push_back(InternalEvent::FindHyperlink(
                    position - self.last_content.terminal_bounds.bounds.origin,
                    false,
                ));
            }
        } else {
            self.last_content.last_hovered_word = None;
        }
    }

    pub fn select_word_at_event_position(&mut self, e: &MouseDownEvent) {
        let position = e.position - self.last_content.terminal_bounds.bounds.origin;
        let (point, side) = grid_point_and_side(
            position,
            self.last_content.terminal_bounds,
            self.last_content.display_offset,
        );
        let selection = Selection::new(SelectionType::Semantic, point, side);
        self.events
            .push_back(InternalEvent::SetSelection(Some((selection, point))));
    }

    pub fn mouse_drag(
        &mut self,
        e: &MouseMoveEvent,
        region: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let position = e.position - self.last_content.terminal_bounds.bounds.origin;
        if !self.mouse_mode(e.modifiers.shift) {
            self.selection_phase = SelectionPhase::Selecting;
            // Alacritty has the same ordering, of first updating the selection
            // then scrolling 15ms later
            self.events
                .push_back(InternalEvent::UpdateSelection(position));

            // Doesn't make sense to scroll the alt screen
            if !self.last_content.mode.contains(TermMode::ALT_SCREEN) {
                let scroll_lines = match self.drag_line_delta(e, region) {
                    Some(value) => value,
                    None => return,
                };

                self.events
                    .push_back(InternalEvent::Scroll(aScroll::Delta(scroll_lines)));
            }

            cx.notify();
        }
    }

    fn drag_line_delta(&self, e: &MouseMoveEvent, region: Bounds<Pixels>) -> Option<i32> {
        let top = region.origin.y;
        let bottom = region.bottom_left().y;

        let scroll_lines = if e.position.y < top {
            let scroll_delta = (top - e.position.y).pow(1.1);
            (scroll_delta / self.last_content.terminal_bounds.line_height).ceil() as i32
        } else if e.position.y > bottom {
            let scroll_delta = -((e.position.y - bottom).pow(1.1));
            (scroll_delta / self.last_content.terminal_bounds.line_height).floor() as i32
        } else {
            return None;
        };

        Some(scroll_lines.clamp(-3, 3))
    }

    pub fn mouse_down(&mut self, e: &MouseDownEvent, _cx: &mut Context<Self>) {
        let position = e.position - self.last_content.terminal_bounds.bounds.origin;
        let point = grid_point(
            position,
            self.last_content.terminal_bounds,
            self.last_content.display_offset,
        );

        if self.mouse_mode(e.modifiers.shift) {
            if let Some(bytes) =
                mouse_button_report(point, e.button, e.modifiers, true, self.last_content.mode)
            {
                self.write_to_pty(bytes);
            }
        } else {
            match e.button {
                MouseButton::Left => {
                    let (point, side) = grid_point_and_side(
                        position,
                        self.last_content.terminal_bounds,
                        self.last_content.display_offset,
                    );

                    let selection_type = match e.click_count {
                        0 => return, //This is a release
                        1 => Some(SelectionType::Simple),
                        2 => Some(SelectionType::Semantic),
                        3 => Some(SelectionType::Lines),
                        _ => None,
                    };

                    if selection_type == Some(SelectionType::Simple) && e.modifiers.shift {
                        self.events
                            .push_back(InternalEvent::UpdateSelection(position));
                        return;
                    }

                    let selection = selection_type
                        .map(|selection_type| Selection::new(selection_type, point, side));

                    if let Some(sel) = selection {
                        self.events
                            .push_back(InternalEvent::SetSelection(Some((sel, point))));
                    }
                }
                #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                MouseButton::Middle => {
                    if let Some(item) = _cx.read_from_primary() {
                        let text = item.text().unwrap_or_default();
                        self.input(text.into_bytes());
                    }
                }
                _ => {}
            }
        }
    }

    pub fn mouse_up(&mut self, e: &MouseUpEvent, cx: &Context<Self>) {
        // let setting = TerminalSettings::get_global(cx);

        let position = e.position - self.last_content.terminal_bounds.bounds.origin;
        if self.mouse_mode(e.modifiers.shift) {
            let point = grid_point(
                position,
                self.last_content.terminal_bounds,
                self.last_content.display_offset,
            );

            if let Some(bytes) =
                mouse_button_report(point, e.button, e.modifiers, false, self.last_content.mode)
            {
                self.write_to_pty(bytes);
            }
        } else {
            // if e.button == MouseButton::Left && setting.copy_on_select {
            //     self.copy(Some(true));
            // }

            //Hyperlinks
            if self.selection_phase == SelectionPhase::Ended {
                let mouse_cell_index =
                    content_index_for_mouse(position, &self.last_content.terminal_bounds);
                if let Some(link) = self.last_content.cells[mouse_cell_index].hyperlink() {
                    cx.open_url(link.uri());
                } else if e.modifiers.secondary() {
                    self.events
                        .push_back(InternalEvent::FindHyperlink(position, true));
                }
            }
        }

        self.selection_phase = SelectionPhase::Ended;
        self.last_mouse = None;
    }

    ///Scroll the terminal
    pub fn scroll_wheel(&mut self, e: &ScrollWheelEvent, scroll_multiplier: f32) {
        let mouse_mode = self.mouse_mode(e.shift);
        let scroll_multiplier = if mouse_mode { 1. } else { scroll_multiplier };

        if let Some(scroll_lines) = self.determine_scroll_lines(e, scroll_multiplier) {
            if mouse_mode {
                let point = grid_point(
                    e.position - self.last_content.terminal_bounds.bounds.origin,
                    self.last_content.terminal_bounds,
                    self.last_content.display_offset,
                );

                if let Some(scrolls) = scroll_report(point, scroll_lines, e, self.last_content.mode)
                {
                    for scroll in scrolls {
                        self.write_to_pty(scroll);
                    }
                };
            } else if self
                .last_content
                .mode
                .contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL)
                && !e.shift
            {
                self.write_to_pty(alt_scroll(scroll_lines));
            } else if scroll_lines != 0 {
                let scroll = aScroll::Delta(scroll_lines);

                self.events.push_back(InternalEvent::Scroll(scroll));
            }
        }
    }

    fn refresh_hovered_word(&mut self, window: &Window) {
        self.word_from_position(window.mouse_position());
    }

    fn determine_scroll_lines(
        &mut self,
        e: &ScrollWheelEvent,
        scroll_multiplier: f32,
    ) -> Option<i32> {
        let line_height = self.last_content.terminal_bounds.line_height;
        match e.touch_phase {
            /* Reset scroll state on started */
            TouchPhase::Started => {
                self.scroll_px = px(0.);
                None
            }
            /* Calculate the appropriate scroll lines */
            TouchPhase::Moved => {
                let old_offset = (self.scroll_px / line_height) as i32;

                self.scroll_px += e.delta.pixel_delta(line_height).y * scroll_multiplier;

                let new_offset = (self.scroll_px / line_height) as i32;

                // Whenever we hit the edges, reset our stored scroll to 0
                // so we can respond to changes in direction quickly
                self.scroll_px %= self.last_content.terminal_bounds.height();

                Some(new_offset - old_offset)
            }
            TouchPhase::Ended => None,
        }
    }

    pub fn find_matches(
        &self,
        mut searcher: RegexSearch,
        cx: &Context<Self>,
    ) -> Task<Vec<RangeInclusive<aPoint>>> {
        let term = self.term.clone();
        cx.background_spawn(async move {
            let term = term.lock();

            all_search_matches(&term, &mut searcher).collect()
        })
    }

    pub fn working_directory(&self) -> Option<PathBuf> {
        if self.is_remote_terminal {
            // We can't yet reliably detect the working directory of a shell on the
            // SSH host. Until we can do that, it doesn't make sense to display
            // the working directory on the client and persist that.
            None
        } else {
            self.client_side_working_directory()
        }
    }

    /// Returns the working directory of the process that's connected to the PTY.
    /// That means it returns the working directory of the local shell or program
    /// that's running inside the terminal.
    ///
    /// This does *not* return the working directory of the shell that runs on the
    /// remote host, in case Zed is connected to a remote host.
    fn client_side_working_directory(&self) -> Option<PathBuf> {
        match &self.terminal_type {
            TerminalType::Pty { info, .. } => {
                info.current.as_ref().map(|process| process.cwd.clone())
            }
            TerminalType::DisplayOnly => None,
        }
    }

    // pub fn title(&self, truncate: bool) -> String {
    //     const MAX_CHARS: usize = 25;
    //     match &self.task {
    //         Some(task_state) => {
    //             if truncate {
    //                 truncate_and_trailoff(&task_state.spawned_task.label, MAX_CHARS)
    //             } else {
    //                 task_state.spawned_task.full_label.clone()
    //             }
    //         }
    //         None => self
    //             .title_override
    //             .as_ref()
    //             .map(|title_override| title_override.to_string())
    //             .unwrap_or_else(|| match &self.terminal_type {
    //                 TerminalType::Pty { info, .. } => info
    //                     .current
    //                     .as_ref()
    //                     .map(|fpi| {
    //                         let process_file = fpi
    //                             .cwd
    //                             .file_name()
    //                             .map(|name| name.to_string_lossy().into_owned())
    //                             .unwrap_or_default();

    //                         let argv = fpi.argv.as_slice();
    //                         let process_name = format!(
    //                             "{}{}",
    //                             fpi.name,
    //                             if !argv.is_empty() {
    //                                 format!(" {}", (argv[1..]).join(" "))
    //                             } else {
    //                                 "".to_string()
    //                             }
    //                         );
    //                         let (process_file, process_name) = if truncate {
    //                             (
    //                                 truncate_and_trailoff(&process_file, MAX_CHARS),
    //                                 truncate_and_trailoff(&process_name, MAX_CHARS),
    //                             )
    //                         } else {
    //                             (process_file, process_name)
    //                         };
    //                         format!("{process_file} — {process_name}")
    //                     })
    //                     .unwrap_or_else(|| "Terminal".to_string()),
    //                 TerminalType::DisplayOnly => "Terminal".to_string(),
    //             }),
    //     }
    // }

    // pub fn kill_active_task(&mut self) {
    //     if let Some(task) = self.task()
    //         && task.status == TaskStatus::Running
    //     {
    //         if let TerminalType::Pty { info, .. } = &mut self.terminal_type {
    //             info.kill_current_process();
    //         }
    //     }
    // }

    pub fn pid(&self) -> Option<sysinfo::Pid> {
        match &self.terminal_type {
            TerminalType::Pty { info, .. } => info.pid(),
            TerminalType::DisplayOnly => None,
        }
    }

    pub fn pid_getter(&self) -> Option<&ProcessIdGetter> {
        match &self.terminal_type {
            TerminalType::Pty { info, .. } => Some(info.pid_getter()),
            TerminalType::DisplayOnly => None,
        }
    }

    // pub fn task(&self) -> Option<&TaskState> {
    //     self.task.as_ref()
    // }

    // pub fn wait_for_completed_task(&self, cx: &App) -> Task<Option<ExitStatus>> {
    //     if let Some(task) = self.task() {
    //         if task.status == TaskStatus::Running {
    //             let completion_receiver = task.completion_rx.clone();
    //             return cx.spawn(async move |_| completion_receiver.recv().await.ok().flatten());
    //         } else if let Ok(status) = task.completion_rx.try_recv() {
    //             return Task::ready(status);
    //         }
    //     }
    //     Task::ready(None)
    // }

    // fn register_task_finished(&mut self, error_code: Option<i32>, cx: &mut Context<Terminal>) {
    //     let e: Option<ExitStatus> = error_code.map(|code| {
    //         #[cfg(unix)]
    //         {
    //             std::os::unix::process::ExitStatusExt::from_raw(code)
    //         }
    //         #[cfg(windows)]
    //         {
    //             std::os::windows::process::ExitStatusExt::from_raw(code as u32)
    //         }
    //     });

    //     if let Some(tx) = &self.completion_tx {
    //         tx.try_send(e).ok();
    //     }
    //     if let Some(e) = e {
    //         self.child_exited = Some(e);
    //     }
    //     let task = match &mut self.task {
    //         Some(task) => task,
    //         None => {
    //             if self.child_exited.is_none_or(|e| e.code() == Some(0)) {
    //                 cx.emit(Event::CloseTerminal);
    //             }
    //             return;
    //         }
    //     };
    //     if task.status != TaskStatus::Running {
    //         return;
    //     }
    //     match error_code {
    //         Some(error_code) => {
    //             task.status.register_task_exit(error_code);
    //         }
    //         None => {
    //             task.status.register_terminal_exit();
    //         }
    //     };

    //     let (finished_successfully, task_line, command_line) = task_summary(task, error_code);
    //     let mut lines_to_show = Vec::new();
    //     if task.spawned_task.show_summary {
    //         lines_to_show.push(task_line.as_str());
    //     }
    //     if task.spawned_task.show_command {
    //         lines_to_show.push(command_line.as_str());
    //     }

    //     if !lines_to_show.is_empty() {
    //         // SAFETY: the invocation happens on non `TaskStatus::Running` tasks, once,
    //         // after either `aEvent::Exit` or `aEvent::ChildExit` events that are spawned
    //         // when Zed task finishes and no more output is made.
    //         // After the task summary is output once, no more text is appended to the terminal.
    //         unsafe { append_text_to_term(&mut self.term.lock(), &lines_to_show) };
    //     }

    //     match task.spawned_task.hide {
    //         HideStrategy::Never => {}
    //         HideStrategy::Always => {
    //             cx.emit(Event::CloseTerminal);
    //         }
    //         HideStrategy::OnSuccess => {
    //             if finished_successfully {
    //                 cx.emit(Event::CloseTerminal);
    //             }
    //         }
    //     }
    // }

    pub fn vi_mode_enabled(&self) -> bool {
        self.vi_mode_enabled
    }

    // pub fn clone_builder(&self, cx: &App, cwd: Option<PathBuf>) -> Task<Result<TerminalBuilder>> {
    //     let working_directory = self.working_directory().or_else(|| cwd);
    //     TerminalBuilder::new(
    //         working_directory,
    //         None,
    //         self.template.shell.clone(),
    //         self.template.env.clone(),
    //         self.template.cursor_shape,
    //         self.template.alternate_scroll,
    //         self.template.max_scroll_history_lines,
    //         self.template.path_hyperlink_regexes.clone(),
    //         self.template.path_hyperlink_timeout_ms,
    //         self.is_remote_terminal,
    //         self.template.window_id,
    //         None,
    //         cx,
    //         self.activation_script.clone(),
    //     )
    // }
}
// Helper function to convert a grid row to a string
pub fn row_to_string(row: &Row<Cell>) -> String {
    row[..Column(row.len())]
        .iter()
        .map(|cell| cell.c)
        .collect::<String>()
}

fn make_selection(range: &RangeInclusive<aPoint>) -> Selection {
    let mut selection = Selection::new(SelectionType::Simple, *range.start(), aDirection::Left);
    selection.update(*range.end(), aDirection::Right);
    selection
}

fn all_search_matches<'a, T>(
    term: &'a Term<T>,
    regex: &'a mut RegexSearch,
) -> impl Iterator<Item = Match> + 'a {
    let start = aPoint::new(term.grid().topmost_line(), Column(0));
    let end = aPoint::new(term.grid().bottommost_line(), term.grid().last_column());
    RegexIter::new(start, end, aDirection::Right, term, regex)
}

fn content_index_for_mouse(pos: Point<Pixels>, terminal_bounds: &TerminalBounds) -> usize {
    let col = (pos.x / terminal_bounds.cell_width()).round() as usize;
    let clamped_col = min(col, terminal_bounds.columns() - 1);
    let row = (pos.y / terminal_bounds.line_height()).round() as usize;
    let clamped_row = min(row, terminal_bounds.screen_lines() - 1);
    clamped_row * terminal_bounds.columns() + clamped_col
}

/// Converts an 8 bit ANSI color to its GPUI equivalent.
/// Accepts `usize` for compatibility with the `alacritty::Colors` interface,
/// Other than that use case, should only be called with values in the `[0,255]` range
pub fn get_color_at_index(index: usize, theme: &Theme) -> Hsla {
    let colors = theme.colors;

    match index {
        // // 0-15 are the same as the named colors above
        // 0 => colors.terminal_ansi_black,
        // 1 => colors.terminal_ansi_red,
        // 2 => colors.terminal_ansi_green,
        // 3 => colors.terminal_ansi_yellow,
        // 4 => colors.terminal_ansi_blue,
        // 5 => colors.terminal_ansi_magenta,
        // 6 => colors.terminal_ansi_cyan,
        // 7 => colors.terminal_ansi_white,
        // 8 => colors.terminal_ansi_bright_black,
        // 9 => colors.terminal_ansi_bright_red,
        // 10 => colors.terminal_ansi_bright_green,
        // 11 => colors.terminal_ansi_bright_yellow,
        // 12 => colors.terminal_ansi_bright_blue,
        // 13 => colors.terminal_ansi_bright_magenta,
        // 14 => colors.terminal_ansi_bright_cyan,
        // 15 => colors.terminal_ansi_bright_white,
        // // 16-231 are a 6x6x6 RGB color cube, mapped to 0-255 using steps defined by XTerm.
        // // See: https://github.com/xterm-x11/xterm-snapshots/blob/master/256colres.pl
        // 16..=231 => {
        //     let (r, g, b) = rgb_for_index(index as u8);
        //     rgba_color(
        //         if r == 0 { 0 } else { r * 40 + 55 },
        //         if g == 0 { 0 } else { g * 40 + 55 },
        //         if b == 0 { 0 } else { b * 40 + 55 },
        //     )
        // }
        // // 232-255 are a 24-step grayscale ramp from (8, 8, 8) to (238, 238, 238).
        // 232..=255 => {
        //     let i = index as u8 - 232; // Align index to 0..24
        //     let value = i * 10 + 8;
        //     rgba_color(value, value, value)
        // }
        // // For compatibility with the alacritty::Colors interface
        // // See: https://github.com/alacritty/alacritty/blob/master/alacritty_terminal/src/term/color.rs
        // 256 => colors.terminal_foreground,
        // 257 => colors.terminal_background,
        // 258 => theme.players().local().cursor,
        // 259 => colors.terminal_ansi_dim_black,
        // 260 => colors.terminal_ansi_dim_red,
        // 261 => colors.terminal_ansi_dim_green,
        // 262 => colors.terminal_ansi_dim_yellow,
        // 263 => colors.terminal_ansi_dim_blue,
        // 264 => colors.terminal_ansi_dim_magenta,
        // 265 => colors.terminal_ansi_dim_cyan,
        // 266 => colors.terminal_ansi_dim_white,
        // 267 => colors.terminal_bright_foreground,
        // 268 => colors.terminal_ansi_black, // 'Dim Background', non-standard color

        _ => black(),
    }
}

/// Generates the RGB channels in [0, 5] for a given index into the 6x6x6 ANSI color cube.
///
/// See: [8 bit ANSI color](https://en.wikipedia.org/wiki/ANSI_escape_code#8-bit).
///
/// Wikipedia gives a formula for calculating the index for a given color:
///
/// ```text
/// index = 16 + 36 × r + 6 × g + b (0 ≤ r, g, b ≤ 5)
/// ```
///
/// This function does the reverse, calculating the `r`, `g`, and `b` components from a given index.
fn rgb_for_index(i: u8) -> (u8, u8, u8) {
    debug_assert!((16..=231).contains(&i));
    let i = i - 16;
    let r = (i - (i % 36)) / 36;
    let g = ((i % 36) - (i % 6)) / 6;
    let b = (i % 36) % 6;
    (r, g, b)
}

pub fn rgba_color(r: u8, g: u8, b: u8) -> Hsla {
    Rgba {
        r: (r as f32 / 255.),
        g: (g as f32 / 255.),
        b: (b as f32 / 255.),
        a: 1.,
    }
    .into()
}