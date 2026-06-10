mod mappings;

mod alacritty;
mod pty_info;
pub mod terminal_settings;

use anyhow::{Result, bail};
use futures_lite::future::yield_now;
use log::trace;

use futures::{
    FutureExt,
    channel::mpsc::{UnboundedReceiver, unbounded},
};

use itertools::Itertools as _;
use mappings::mouse::{
    alt_scroll, grid_point, grid_point_and_side, mouse_button_report, mouse_moved_report,
    scroll_report,
};

use async_channel::Sender;
use futures::StreamExt;
use pty_info::{ProcessIdGetter, PtyProcessInfo};
use serde::{Deserialize, Serialize};
use settings::Settings;
use std::collections::{HashMap, VecDeque};
use terminal_settings::{AlternateScroll, CursorShape as SettingsCursorShape, TerminalSettings};
// use theme::{ActiveTheme, Theme};
use urlencoding;
use util::{paths::PathStyle, shell::Shell, truncate_and_trailoff};

use std::{
    borrow::Cow,
    cmp::{self, min},
    fmt::{self, Display, Formatter},
    ops::{BitOr, BitOrAssign, Deref, Range as StdRange},
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::Arc,
    time::{Duration, Instant},
};
use thiserror::Error;
use vte::ansi::{Attr, Handler, Processor, StdSyncHandler};
pub use vte::ansi::{Color, NamedColor, Rgb};

use gpui::{
    App, AppContext as _, BackgroundExecutor, Bounds, ClipboardItem, Context, EventEmitter, Hsla,
    Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point as GpuiPoint, Rgba, ScrollWheelEvent, Size, Task, TouchPhase, Window, actions, black, px,
};

use crate::alacritty::{
    AlacrittyCell, AlacrittyGridIterator, AlacrittyHyperlink, AlacrittySearch, AlacrittyTerm,
    AlacrittyTermConfig, AlacrittyTermLock, HyperlinkMatch, PtySender, RegexSearches, apply_config,
    clear_saved_screen, content_text, display_offset, display_only_term_config,
    find_from_terminal_point, full_content_range, last_non_empty_lines, make_content, new_term,
    open_pty, pty_options, pty_term_config, resize, screen_lines, scroll_display, scroll_to_point,
    search_matches, selection_text, set_default_cursor_style, set_selection as set_term_selection,
    spawn_event_loop, toggle_vi_mode as toggle_term_vi_mode, total_lines,
    update_selection as update_term_selection, update_selection_to_vi_cursor,
    update_vi_cursor_for_scroll, vi_goto_point, vi_motion,
};
use crate::mappings::colors::to_vte_rgb;
use crate::mappings::keys::to_esc_str;

#[derive(Clone, Copy, Debug)]
enum Scroll {
    Delta(i32),
    PageUp,
    PageDown,
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug)]
enum ViMotion {
    Up,
    Down,
    Left,
    Right,
    First,
    Last,
    FirstOccupied,
    High,
    Middle,
    Low,
    WordLeft,
    WordRight,
    WordRightEnd,
    Bracket,
}

#[derive(Clone, Debug)]
pub struct Search {
    search: AlacrittySearch,
}

#[derive(Clone, Debug)]
struct Selection {
    ty: SelectionType,
    start: SelectionAnchor,
    end: SelectionAnchor,
    head: Point,
}

#[derive(Clone, Copy, Debug)]
struct SelectionAnchor {
    point: Point,
    side: SelectionSide,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SelectionSide {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SelectionType {
    Simple,
    Semantic,
    Lines,
}

impl Selection {
    fn new(selection_type: SelectionType, point: Point, side: SelectionSide) -> Self {
        let anchor = SelectionAnchor { point, side };
        Self {
            ty: selection_type,
            start: anchor,
            end: anchor,
            head: point,
        }
    }

    fn simple_range(range: Range) -> Self {
        let mut selection = Self::new(SelectionType::Simple, range.start(), SelectionSide::Left);
        selection.update(range.end(), SelectionSide::Right);
        selection
    }

    fn update(&mut self, point: Point, side: SelectionSide) {
        self.end = SelectionAnchor { point, side };
        self.head = point;
    }
}

pub fn is_default_background_color(color: Color) -> bool {
    matches!(color, Color::Named(NamedColor::Background))
}

pub fn is_app_chosen_exact_color(color: Color) -> bool {
    matches!(color, Color::Spec(_) | Color::Indexed(16..=255))
}

pub type AnsiSpans = Vec<(StdRange<usize>, Option<Color>)>;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParsedAnsiText {
    pub text: String,
    pub foreground_spans: AnsiSpans,
    pub background_spans: AnsiSpans,
}

pub fn parse_ansi_text(input: &[u8]) -> ParsedAnsiText {
    let mut handler = StyledAnsiTextHandler::default();
    let mut processor = Processor::<StdSyncHandler>::default();
    processor.advance(&mut handler, input);
    handler.finish()
}

pub fn strip_ansi_text(input: &[u8]) -> String {
    let mut handler = PlainAnsiTextHandler::default();
    let mut processor = Processor::<StdSyncHandler>::default();
    processor.advance(&mut handler, input);
    handler.text
}

#[derive(Default)]
struct StyledAnsiTextHandler {
    text: String,
    foreground_spans: AnsiSpans,
    background_spans: AnsiSpans,
    current_foreground_range_start: usize,
    current_background_range_start: usize,
    current_foreground_color: Option<Color>,
    current_background_color: Option<Color>,
}

impl StyledAnsiTextHandler {
    fn finish(mut self) -> ParsedAnsiText {
        if self.current_foreground_range_start < self.text.len() {
            self.foreground_spans.push((
                self.current_foreground_range_start..self.text.len(),
                self.current_foreground_color,
            ));
        }

        if self.current_background_range_start < self.text.len() {
            self.background_spans.push((
                self.current_background_range_start..self.text.len(),
                self.current_background_color,
            ));
        }

        ParsedAnsiText {
            text: self.text,
            foreground_spans: self.foreground_spans,
            background_spans: self.background_spans,
        }
    }

    fn break_foreground_span(&mut self, color: Option<Color>) {
        self.foreground_spans.push((
            self.current_foreground_range_start..self.text.len(),
            self.current_foreground_color,
        ));
        self.current_foreground_color = color;
        self.current_foreground_range_start = self.text.len();
    }

    fn break_background_span(&mut self, color: Option<Color>) {
        self.background_spans.push((
            self.current_background_range_start..self.text.len(),
            self.current_background_color,
        ));
        self.current_background_color = color;
        self.current_background_range_start = self.text.len();
    }
}

impl Handler for StyledAnsiTextHandler {
    fn input(&mut self, c: char) {
        self.text.push(c);
    }

    fn linefeed(&mut self) {
        self.text.push('\n');
    }

    fn put_tab(&mut self, count: u16) {
        self.text.extend(std::iter::repeat_n('\t', count as usize));
    }

    fn terminal_attribute(&mut self, attr: Attr) {
        match attr {
            Attr::Foreground(color) => {
                self.break_foreground_span(Some(color));
            }
            Attr::Background(color) => {
                self.break_background_span(Some(color));
            }
            Attr::Reset => {
                self.break_foreground_span(None);
                self.break_background_span(None);
            }
            _ => {}
        }
    }
}

#[derive(Default)]
struct PlainAnsiTextHandler {
    text: String,
    line_start: usize,
}

impl Handler for PlainAnsiTextHandler {
    fn input(&mut self, c: char) {
        self.text.push(c);
    }

    fn linefeed(&mut self) {
        self.text.push('\n');
        self.line_start = self.text.len();
    }

    fn carriage_return(&mut self) {
        self.text.truncate(self.line_start);
    }

    fn put_tab(&mut self, count: u16) {
        self.text.extend(std::iter::repeat_n('\t', count as usize));
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Hyperlink {
    data: HyperlinkData,
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum HyperlinkData {
    Alacritty(AlacrittyHyperlink),
    Owned { id: Option<Arc<str>>, uri: Arc<str> },
}

#[derive(Default, Debug, Clone, Eq, PartialEq)]
pub struct Cell {
    cell: AlacrittyCell,
}

pub struct RenderableCells<'a> {
    cells: AlacrittyGridIterator<'a>,
}

#[derive(Debug, Clone)]
pub struct IndexedCell {
    pub point: Point,
    pub cell: Cell,
}

impl Deref for IndexedCell {
    type Target = Cell;

    #[inline]
    fn deref(&self) -> &Cell {
        &self.cell
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Modes(u32);

impl Modes {
    pub const NONE: Self = Self(0);
    pub const APP_CURSOR: Self = Self(1 << 0);
    pub const APP_KEYPAD: Self = Self(1 << 1);
    pub const SHOW_CURSOR: Self = Self(1 << 2);
    pub const LINE_WRAP: Self = Self(1 << 3);
    pub const ORIGIN: Self = Self(1 << 4);
    pub const INSERT: Self = Self(1 << 5);
    pub const LINE_FEED_NEW_LINE: Self = Self(1 << 6);
    pub const FOCUS_IN_OUT: Self = Self(1 << 7);
    pub const ALTERNATE_SCROLL: Self = Self(1 << 8);
    pub const BRACKETED_PASTE: Self = Self(1 << 9);
    pub const SGR_MOUSE: Self = Self(1 << 10);
    pub const UTF8_MOUSE: Self = Self(1 << 11);
    pub const ALT_SCREEN: Self = Self(1 << 12);
    pub const MOUSE_REPORT_CLICK: Self = Self(1 << 13);
    pub const MOUSE_DRAG: Self = Self(1 << 14);
    pub const MOUSE_MOTION: Self = Self(1 << 15);
    pub const VI: Self = Self(1 << 16);
    pub const MOUSE_MODE: Self =
        Self(Self::MOUSE_REPORT_CLICK.0 | Self::MOUSE_DRAG.0 | Self::MOUSE_MOTION.0);

    pub const fn empty() -> Self {
        Self::NONE
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    pub fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }
}

impl BitOr for Modes {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for Modes {
    fn bitor_assign(&mut self, rhs: Self) {
        self.insert(rhs);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cursor {
    pub shape: CursorShape,
    pub point: Point,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorShape {
    Block,
    Underline,
    Bar,
    HollowBlock,
    Hidden,
}

impl From<SettingsCursorShape> for CursorShape {
    fn from(shape: SettingsCursorShape) -> Self {
        match shape {
            SettingsCursorShape::Block => Self::Block,
            SettingsCursorShape::Underline => Self::Underline,
            SettingsCursorShape::Bar => Self::Bar,
            SettingsCursorShape::Hollow => Self::HollowBlock,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Point {
    pub line: i32,
    pub column: usize,
}

impl Point {
    pub fn new(line: i32, column: usize) -> Self {
        Self { line, column }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Range {
    start: Point,
    end: Point,
}

impl Range {
    pub fn new(start: Point, end: Point) -> Self {
        Self { start, end }
    }

    pub fn start(&self) -> Point {
        self.start
    }

    pub fn end(&self) -> Point {
        self.end
    }

    pub fn contains(&self, point: Point) -> bool {
        self.start <= point && point <= self.end
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelectionRange {
    pub start: Point,
    pub end: Point,
    pub is_block: bool,
}

impl SelectionRange {
    pub fn point_range(self) -> Range {
        Range::new(self.start, self.end)
    }
}

// TODO: Un-pub
#[derive(Clone)]
pub struct Content {
    pub cells: Vec<IndexedCell>,
    pub mode: Modes,
    pub display_offset: usize,
    pub selection_text: Option<String>,
    pub selection: Option<SelectionRange>,
    pub cursor: Cursor,
    pub cursor_char: char,
    pub terminal_bounds: TerminalBounds,
    pub last_hovered_word: Option<HoveredWord>,
    pub scrolled_to_top: bool,
    pub scrolled_to_bottom: bool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct HoveredWord {
    pub word: String,
    pub word_match: Range,
    pub id: usize,
}

impl Default for Content {
    fn default() -> Self {
        Content {
            cells: Default::default(),
            mode: Default::default(),
            display_offset: Default::default(),
            selection_text: Default::default(),
            selection: Default::default(),
            cursor: Cursor {
                shape: CursorShape::Block,
                point: Point::new(0, 0),
            },
            cursor_char: Default::default(),
            terminal_bounds: Default::default(),
            last_hovered_word: None,
            scrolled_to_top: false,
            scrolled_to_bottom: false,
        }
    }
}

#[derive(PartialEq, Eq)]
enum SelectionPhase {
    Selecting,
    Ended,
}

#[cfg(test)]
mod domain_tests {
    use super::*;

    #[test]
    fn strip_ansi_text_removes_ansi_and_handles_carriage_returns() {
        let cases = [
            ("no escape codes here\n", "no escape codes here\n"),
            ("\x1b[31mhello\x1b[0m", "hello"),
            ("\x1b[1;32mfoo\x1b[0m bar", "foo bar"),
            ("progress 10%\rprogress 100%\n", "progress 100%\n"),
        ];

        for (input, expected) in cases {
            assert_eq!(strip_ansi_text(input.as_bytes()), expected);
        }
    }

    #[test]
    fn parse_ansi_text_records_foreground_and_background_spans() {
        let parsed = parse_ansi_text(b"\x1b[31mred\x1b[44mblue-bg\x1b[0mplain");

        assert_eq!(parsed.text, "redblue-bgplain");
        assert_eq!(
            parsed.foreground_spans,
            vec![
                (0..0, None),
                (0..10, Some(Color::Named(NamedColor::Red))),
                (10..15, None),
            ]
        );
        assert_eq!(
            parsed.background_spans,
            vec![
                (0..3, None),
                (3..10, Some(Color::Named(NamedColor::Blue))),
                (10..15, None),
            ]
        );
    }

    #[test]
    fn terminal_cell_clone_shares_extra_storage() {
        let mut cell = Cell::default();
        cell.push_zerowidth('a');

        let clone = cell.clone();

        match (&cell.cell.extra, &clone.cell.extra) {
            (Some(extra), Some(clone_extra)) => assert!(Arc::ptr_eq(extra, clone_extra)),
            _ => panic!("expected extra storage on both cells"),
        }
    }
}

actions!(
    terminal,
    [
        /// Clears the terminal screen.
        Clear,
        /// Copies selected text to the clipboard.
        Copy,
        /// Pastes from the clipboard.
        Paste,
        /// Pastes the text from the clipboard.
        PasteText,
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

/// Inserts Zed-specific environment variables for terminal sessions.
/// Used by both local terminals and remote terminals (via SSH).
pub fn insert_zed_terminal_env(env: &mut HashMap<String, String>) {
    env.insert("ZED_TERM".to_string(), "true".to_string());
    env.insert("TERM_PROGRAM".to_string(), "zed".to_string());
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    env.insert("COLORTERM".to_string(), "truecolor".to_string());
}

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
    /// HTTP, git, etc. string determined by the `URL_REGEX` regex.
    Url(String),
    /// File system path, absolute or relative, existing or not.
    /// Might have line and column number(s) attached as `file.rs:1:23`
    PathLike(PathLikeTarget),
}

#[derive(Clone)]
enum InternalEvent {
    Resize(TerminalBounds),
    Clear,
    // FocusNextMatch,
    Scroll(Scroll),
    ScrollToPoint(Point),
    SetSelection(Option<Selection>),
    UpdateSelection(GpuiPoint<Pixels>),
    FindHyperlink(GpuiPoint<Pixels>, bool),
    ProcessHyperlink(HyperlinkMatch, bool),
    // Whether keep selection when copy
    Copy(Option<bool>),
    // Vi mode events
    ToggleViMode,
    ViMotion(ViMotion),
    MoveViCursorToPoint(Point),
}

type ClipboardFormatter = Arc<dyn Fn(&str) -> String + Sync + Send + 'static>;
type ColorFormatter = Arc<dyn Fn(Rgb) -> String + Sync + Send + 'static>;
type TextAreaSizeFormatter = Arc<dyn Fn(TerminalBounds) -> String + Sync + Send + 'static>;

#[derive(Clone)]
pub(crate) enum TerminalBackendEvent {
    MouseCursorDirty,
    Title(String),
    ResetTitle,
    ClipboardStore(String),
    ClipboardLoad(ClipboardFormatter),
    ColorRequest(usize, ColorFormatter),
    PtyWrite(String),
    TextAreaSizeRequest(TextAreaSizeFormatter),
    CursorBlinkingChange,
    Wakeup,
    Bell,
    Exit,
    ChildExit(ExitStatus),
}

impl fmt::Debug for TerminalBackendEvent {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::MouseCursorDirty => f.write_str("MouseCursorDirty"),
            Self::Title(title) => write!(f, "Title({title})"),
            Self::ResetTitle => f.write_str("ResetTitle"),
            Self::ClipboardStore(data) => write!(f, "ClipboardStore({data})"),
            Self::ClipboardLoad(_) => f.write_str("ClipboardLoad"),
            Self::ColorRequest(index, _) => write!(f, "ColorRequest({index})"),
            Self::PtyWrite(output) => write!(f, "PtyWrite({output})"),
            Self::TextAreaSizeRequest(_) => f.write_str("TextAreaSizeRequest"),
            Self::CursorBlinkingChange => f.write_str("CursorBlinkingChange"),
            Self::Wakeup => f.write_str("Wakeup"),
            Self::Bell => f.write_str("Bell"),
            Self::Exit => f.write_str("Exit"),
            Self::ChildExit(status) => write!(f, "ChildExit({status})"),
        }
    }
}

enum PtyEvent {
    Event(TerminalBackendEvent),
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
        // Tolerance to prevent f32 precision from losing a row:
        // `N * line_height / line_height` can be N-epsilon, which floor()
        // would round down, pushing the first line into invisible scrollback.
        let raw = self.bounds.size.height / self.line_height;
        raw.next_up().floor() as usize
    }

    pub fn num_columns(&self) -> usize {
        let raw = self.bounds.size.width / self.cell_width;
        raw.next_up().floor() as usize
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
                origin: GpuiPoint::default(),
                size: Size {
                    width: DEBUG_TERMINAL_WIDTH,
                    height: DEBUG_TERMINAL_HEIGHT,
                },
            },
        )
    }
}

fn normalize_terminal_bounds(mut bounds: TerminalBounds) -> TerminalBounds {
    bounds.bounds.size.height = cmp::max(bounds.line_height, bounds.height());
    bounds.bounds.size.width = cmp::max(bounds.cell_width, bounds.width());
    bounds
}

#[derive(Error, Debug)]
pub struct TerminalError {
    pub directory: Option<PathBuf>,
    pub program: Option<String>,
    pub args: Option<Vec<String>>,
    pub title_override: Option<String>,
    pub source: std::io::Error,
}

impl TerminalError {
    fn fmt_directory(&self) -> String {
        self.directory
            .clone()
            .map(|path| {
                match path
                    .into_os_string()
                    .into_string()
                    .map_err(|os_str| format!("<non-utf8 path> {}", os_str.to_string_lossy()))
                {
                    Ok(s) => s,
                    Err(s) => s,
                }
            })
            .unwrap_or_else(|| "<none specified>".to_string())
    }

    fn fmt_shell(&self) -> String {
        if let Some(title_override) = &self.title_override {
            format!(
                "{} {} ({})",
                self.program.as_deref().unwrap_or("<system defined shell>"),
                self.args.as_ref().into_iter().flatten().format(" "),
                title_override
            )
        } else {
            format!(
                "{} {}",
                self.program.as_deref().unwrap_or("<system defined shell>"),
                self.args.as_ref().into_iter().flatten().format(" ")
            )
        }
    }
}

impl Display for TerminalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let dir_string: String = self.fmt_directory();
        let shell = self.fmt_shell();

        write!(
            f,
            "Working directory: {} Shell command: `{}`, IOError: {}",
            dir_string, shell, self.source
        )
    }
}

// https://github.com/alacritty/alacritty/blob/cb3a79dbf6472740daca8440d5166c1d4af5029e/extra/man/alacritty.5.scd?plain=1#L207-L213
const DEFAULT_SCROLL_HISTORY_LINES: usize = 10_000;
pub const MAX_SCROLL_HISTORY_LINES: usize = 100_000;

pub struct TerminalBuilder {
    terminal: Terminal,
    events_rx: UnboundedReceiver<PtyEvent>,
}

impl TerminalBuilder {
    pub fn new_display_only(
        cursor_shape: SettingsCursorShape,
        alternate_scroll: AlternateScroll,
        max_scroll_history_lines: Option<usize>,
        window_id: u64,
        background_executor: &BackgroundExecutor,
        path_style: PathStyle,
    ) -> TerminalBuilder {
        Self::new_display_only_with_bounds(
            cursor_shape,
            alternate_scroll,
            max_scroll_history_lines,
            window_id,
            background_executor,
            path_style,
            TerminalBounds::default(),
        )
    }

    pub fn new_display_only_with_bounds(
        cursor_shape: SettingsCursorShape,
        alternate_scroll: AlternateScroll,
        max_scroll_history_lines: Option<usize>,
        window_id: u64,
        background_executor: &BackgroundExecutor,
        path_style: PathStyle,
        terminal_bounds: TerminalBounds,
    ) -> TerminalBuilder {
        let terminal_bounds = normalize_terminal_bounds(terminal_bounds);

        let scrolling_history = max_scroll_history_lines
            .unwrap_or(DEFAULT_SCROLL_HISTORY_LINES)
            .min(MAX_SCROLL_HISTORY_LINES);
        let config = display_only_term_config(scrolling_history, cursor_shape);

        let (events_tx, events_rx) = unbounded();
        let term = new_term(&config, terminal_bounds, events_tx, alternate_scroll);

        let terminal = Terminal {
            terminal_type: TerminalType::DisplayOnly,
            completion_tx: None,
            term,
            term_config: config,
            output_processor: Processor::<StdSyncHandler>::new(),
            title_override: None,
            events: VecDeque::with_capacity(10),
            last_content: Content {
                terminal_bounds,
                ..Default::default()
            },
            last_mouse: None,
            matches: Vec::new(),

            selection_head: None,
            breadcrumb_text: String::new(),
            scroll_px: px(0.),
            next_link_id: 0,
            selection_phase: SelectionPhase::Ended,
            hyperlink_regex_searches: RegexSearches::default(),
            vi_mode_enabled: false,
            is_remote_terminal: false,
            last_mouse_move_time: Instant::now(),
            last_hyperlink_search_position: None,
            mouse_down_hyperlink: None,
            #[cfg(windows)]
            shell_program: None,
            activation_script: Vec::new(),
            template: CopyTemplate {
                shell: Shell::System,
                env: HashMap::default(),
                cursor_shape,
                alternate_scroll,
                max_scroll_history_lines,
                path_hyperlink_regexes: Vec::default(),
                path_hyperlink_timeout_ms: 0,
                window_id,
            },
            child_exited: None,
            keyboard_input_sent: false,
            event_loop_task: Task::ready(Ok(())),
            background_executor: background_executor.clone(),
            path_style,
            #[cfg(any(test, feature = "test-support"))]
            input_log: Vec::new(),
        };

        TerminalBuilder {
            terminal,
            events_rx,
        }
    }

    pub fn new(
        working_directory: Option<PathBuf>,
        shell: Shell,
        mut env: HashMap<String, String>,
        cursor_shape: SettingsCursorShape,
        alternate_scroll: AlternateScroll,
        max_scroll_history_lines: Option<usize>,
        path_hyperlink_regexes: Vec<String>,
        path_hyperlink_timeout_ms: u64,
        is_remote_terminal: bool,
        window_id: u64,
        completion_tx: Option<Sender<Option<ExitStatus>>>,
        cx: &App,
        activation_script: Vec<String>,
        path_style: PathStyle,
    ) -> Task<Result<TerminalBuilder>> {
        let background_executor = cx.background_executor().clone();

        let fut = async move {
            // Remove SHLVL so the spawned shell initializes it to 1, matching
            // the behavior of standalone terminal emulators like iTerm2/Kitty/Alacritty.
            env.remove("SHLVL");

            // If the parent environment doesn't have a locale set
            // (As is the case when launched from a .app on MacOS),
            // and the Project doesn't have a locale set, then
            // set a fallback for our child environment to use.
            if std::env::var("LANG").is_err() {
                env.entry("LANG".to_string())
                    .or_insert_with(|| "en_US.UTF-8".to_string());
            }

            insert_zed_terminal_env(&mut env);

            #[derive(Default)]
            struct ShellParams {
                program: String,
                args: Option<Vec<String>>,
                title_override: Option<String>,
            }

            impl ShellParams {
                fn new(
                    program: String,
                    args: Option<Vec<String>>,
                    title_override: Option<String>,
                ) -> Self {
                    log::debug!("Using {program} as shell");
                    Self {
                        program,
                        args,
                        title_override,
                    }
                }
            }

            let shell_params = match shell.clone() {
                Shell::System => {
                    if cfg!(windows) {
                        Some(ShellParams::new(
                            util::shell::get_windows_system_shell(),
                            None,
                            None,
                        ))
                    } else {
                        None
                    }
                }
                Shell::Program(program) => Some(ShellParams::new(program, None, None)),
                Shell::WithArguments {
                    program,
                    args,
                    title_override,
                } => Some(ShellParams::new(program, Some(args), title_override)),
            };
            let terminal_title_override =
                shell_params.as_ref().and_then(|e| e.title_override.clone());

            #[cfg(windows)]
            let shell_program = shell_params.as_ref().map(|params| {
                use util::ResultExt;

                Self::resolve_path(&params.program)
                    .log_err()
                    .unwrap_or(params.program.clone())
            });

            // Note: when remoting, this shell_kind will scrutinize `ssh` or
            // `wsl.exe` as a shell and fall back to posix or powershell based on
            // the compilation target. This is fine right now due to the restricted
            // way we use the return value, but would become incorrect if we
            // supported remoting into windows.
            let shell_kind = shell.shell_kind(cfg!(windows));

            let alacritty_shell = shell_params.as_ref().map(|params| {
                (
                    params.program.clone(),
                    params.args.clone().unwrap_or_default(),
                )
            });
            let pty_options = pty_options(
                alacritty_shell,
                working_directory.clone(),
                env.clone(),
                // We pass in the foreground thread's signal mask to the child process via pty_options,
                // so terminal construction can run on a background thread without breaking Ctrl-C and other signals
                // otherwise the terminal would inherit the background executor's signal mask which blocks
                // some terminal signals
                #[cfg(windows)]
                shell_kind.tty_escape_args(),
            );

            let scrolling_history = max_scroll_history_lines
                .unwrap_or(DEFAULT_SCROLL_HISTORY_LINES)
                .min(MAX_SCROLL_HISTORY_LINES);
            let config = pty_term_config(scrolling_history, cursor_shape);

            //Setup the pty...
            let pty = match open_pty(&pty_options, TerminalBounds::default(), window_id) {
                Ok(pty) => pty,
                Err(error) => {
                    bail!(TerminalError {
                        directory: working_directory,
                        program: shell_params.as_ref().map(|params| params.program.clone()),
                        args: shell_params.as_ref().and_then(|params| params.args.clone()),
                        title_override: terminal_title_override,
                        source: error,
                    });
                }
            };

            // Spawn a background channel so the Alacritty EventLoop can communicate with us.
            //TODO: Remove with a bounded sender which can be dispatched on &self
            let (events_tx, events_rx) = unbounded();
            //Set up the terminal...
            let term = new_term(
                &config,
                TerminalBounds::default(),
                events_tx.clone(),
                alternate_scroll,
            );

            let pty_info = PtyProcessInfo::new(ProcessIdGetter::from(&pty));

            //And connect them together
            let pty_tx = spawn_event_loop(term.clone(), events_tx, pty, pty_options.drain_on_exit)?;

            let terminal = Terminal {
                terminal_type: TerminalType::Pty {
                    pty_tx,
                    info: Arc::new(pty_info),
                },
                completion_tx,
                term,
                term_config: config,
                output_processor: Processor::<StdSyncHandler>::new(),
                title_override: terminal_title_override,
                events: VecDeque::with_capacity(10), //Should never get this high.
                last_content: Default::default(),
                last_mouse: None,
                matches: Vec::new(),

                selection_head: None,
                breadcrumb_text: String::new(),
                scroll_px: px(0.),
                next_link_id: 0,
                selection_phase: SelectionPhase::Ended,
                hyperlink_regex_searches: RegexSearches::new(
                    &path_hyperlink_regexes,
                    path_hyperlink_timeout_ms,
                ),
                vi_mode_enabled: false,
                is_remote_terminal,
                last_mouse_move_time: Instant::now(),
                last_hyperlink_search_position: None,
                mouse_down_hyperlink: None,
                #[cfg(windows)]
                shell_program,
                activation_script: activation_script.clone(),
                template: CopyTemplate {
                    shell,
                    env,
                    cursor_shape,
                    alternate_scroll,
                    max_scroll_history_lines,
                    path_hyperlink_regexes,
                    path_hyperlink_timeout_ms,
                    window_id,
                },
                child_exited: None,
                keyboard_input_sent: false,
                event_loop_task: Task::ready(Ok(())),
                background_executor,
                path_style,
                #[cfg(any(test, feature = "test-support"))]
                input_log: Vec::new(),
            };

            if !activation_script.is_empty() {
                for activation_script in activation_script {
                    terminal.write_to_pty(activation_script.into_bytes());
                    // Simulate enter key press
                    // NOTE(PowerShell): using `\r\n` will put PowerShell in a continuation mode (infamous >> character)
                    // and generally mess up the rendering.
                    terminal.write_to_pty(b"\x0d");
                }
                // In order to clear the screen at this point, we have two options:
                // 1. We can send a shell-specific command such as "clear" or "cls"
                // 2. We can "echo" a marker message that we will then catch when handling a Wakeup event
                //    and clear the screen using `terminal.clear()` method
                // We cannot issue a `terminal.clear()` command at this point as alacritty is evented
                // and while we have sent the activation script to the pty, it will be executed asynchronously.
                // Therefore, we somehow need to wait for the activation script to finish executing before we
                // can proceed with clearing the screen.
                terminal.write_to_pty(shell_kind.clear_screen_command().as_bytes());
                // Simulate enter key press
                terminal.write_to_pty(b"\x0d");
            }

            Ok(TerminalBuilder {
                terminal,
                events_rx,
            })
        };
        cx.background_spawn(fut)
    }

    pub fn subscribe(mut self, cx: &Context<Terminal>) -> Terminal {
        //Event loop
        self.terminal.event_loop_task = cx.spawn(async move |terminal, cx| {
            while let Some(event) = self.events_rx.next().await {
                terminal.update(cx, |terminal, cx| {
                    //Process the first event immediately for lowered latency
                    terminal.process_pty_event(event, cx);
                })?;

                'outer: loop {
                    let mut events = Vec::new();

                    #[cfg(any(test, feature = "test-support"))]
                    let mut timer = cx.background_executor().simulate_random_delay().fuse();
                    #[cfg(not(any(test, feature = "test-support")))]
                    let mut timer = cx
                        .background_executor()
                        .timer(std::time::Duration::from_millis(4))
                        .fuse();

                    let mut wakeup = false;
                    loop {
                        futures::select_biased! {
                            _ = timer => break,
                            event = self.events_rx.next() => {
                                if let Some(event) = event {
                                    if matches!(event, PtyEvent::Event(TerminalBackendEvent::Wakeup))
                                    {
                                        wakeup = true;
                                    } else {
                                        events.push(event);
                                    }

                                    if events.len() > 100 {
                                        break;
                                    }
                                } else {
                                    break;
                                }
                            },
                        }
                    }

                    if events.is_empty() && !wakeup {
                        yield_now().await;
                        break 'outer;
                    }

                    terminal.update(cx, |this, cx| {
                        if wakeup {
                            this.process_event(TerminalBackendEvent::Wakeup, cx);
                        }

                        for event in events {
                            this.process_pty_event(event, cx);
                        }
                    })?;
                    yield_now().await;
                }
            }
            anyhow::Ok(())
        });
        self.terminal
    }

    #[cfg(windows)]
    fn resolve_path(path: &str) -> Result<String> {
        use windows::Win32::Storage::FileSystem::SearchPathW;
        use windows::core::HSTRING;

        let path = if path.starts_with(r"\\?\") || !path.contains(&['/', '\\']) {
            path.to_string()
        } else {
            r"\\?\".to_string() + path
        };

        let required_length = unsafe { SearchPathW(None, &HSTRING::from(&path), None, None, None) };
        let mut buf = vec![0u16; required_length as usize];
        let size = unsafe { SearchPathW(None, &HSTRING::from(&path), None, Some(&mut buf), None) };

        Ok(String::from_utf16(&buf[..size as usize])?)
    }
}

enum TerminalType {
    Pty {
        pty_tx: PtySender,
        info: Arc<PtyProcessInfo>,
    },
    DisplayOnly,
}

pub struct Terminal {
    terminal_type: TerminalType,
    completion_tx: Option<Sender<Option<ExitStatus>>>,
    term: Arc<AlacrittyTermLock>,
    term_config: AlacrittyTermConfig,
    output_processor: Processor<StdSyncHandler>,
    events: VecDeque<InternalEvent>,
    /// This is only used for mouse mode cell change detection
    last_mouse: Option<(Point, SelectionSide)>,
    pub matches: Vec<Range>,
    pub last_content: Content,
    pub selection_head: Option<Point>,

    pub breadcrumb_text: String,
    title_override: Option<String>,
    scroll_px: Pixels,
    next_link_id: usize,
    selection_phase: SelectionPhase,
    hyperlink_regex_searches: RegexSearches,
    vi_mode_enabled: bool,
    is_remote_terminal: bool,
    last_mouse_move_time: Instant,
    last_hyperlink_search_position: Option<GpuiPoint<Pixels>>,
    mouse_down_hyperlink: Option<HyperlinkMatch>,
    #[cfg(windows)]
    shell_program: Option<String>,
    template: CopyTemplate,
    activation_script: Vec<String>,
    child_exited: Option<ExitStatus>,
    keyboard_input_sent: bool,
    event_loop_task: Task<Result<(), anyhow::Error>>,
    background_executor: BackgroundExecutor,
    path_style: PathStyle,
    #[cfg(any(test, feature = "test-support"))]
    input_log: Vec<Vec<u8>>,
}

struct CopyTemplate {
    shell: Shell,
    env: HashMap<String, String>,
    cursor_shape: SettingsCursorShape,
    alternate_scroll: AlternateScroll,
    max_scroll_history_lines: Option<usize>,
    path_hyperlink_regexes: Vec<String>,
    path_hyperlink_timeout_ms: u64,
    window_id: u64,
}

const FIND_HYPERLINK_THROTTLE_PX: Pixels = px(5.0);

impl Terminal {
    fn process_pty_event(&mut self, event: PtyEvent, cx: &mut Context<Self>) {
        match event {
            PtyEvent::Event(event) => self.process_event(event, cx),
        }
    }

    fn process_event(&mut self, event: TerminalBackendEvent, cx: &mut Context<Self>) {
        match event {
            TerminalBackendEvent::Title(title) => {
                // ignore default shell program title change as windows always sends those events
                // and it would end up showing the shell executable path in breadcrumbs
                #[cfg(windows)]
                if self
                    .shell_program
                    .as_ref()
                    .map(|e| *e == title)
                    .unwrap_or(false)
                {
                    return;
                }

                self.breadcrumb_text = title;
                cx.emit(Event::BreadcrumbsChanged);
            }
            TerminalBackendEvent::ResetTitle => {
                self.breadcrumb_text = String::new();
                cx.emit(Event::BreadcrumbsChanged);
            }
            TerminalBackendEvent::ClipboardStore(data) => {
                cx.write_to_clipboard(ClipboardItem::new_string(data))
            }
            TerminalBackendEvent::ClipboardLoad(format) => {
                self.write_to_pty(
                    match &cx.read_from_clipboard().and_then(|item| item.text()) {
                        // The terminal only supports pasting strings, not images.
                        Some(text) => format(text),
                        _ => format(""),
                    }
                    .into_bytes(),
                )
            }
            TerminalBackendEvent::PtyWrite(out) => self.write_to_pty(out.into_bytes()),
            TerminalBackendEvent::TextAreaSizeRequest(format) => {
                self.write_to_pty(format(self.last_content.terminal_bounds).into_bytes())
            }
            TerminalBackendEvent::CursorBlinkingChange => {
                let terminal = self.term.lock();
                let blinking = terminal.cursor_style().blinking;
                cx.emit(Event::BlinkChanged(blinking));
            }
            TerminalBackendEvent::Bell => {
                cx.emit(Event::Bell);
            }
            TerminalBackendEvent::Exit => self.register_task_finished(None, cx),
            TerminalBackendEvent::MouseCursorDirty => {
                //NOOP, Handled in render
            }
            TerminalBackendEvent::Wakeup => {
                cx.emit(Event::Wakeup);

                if let TerminalType::Pty { info, .. } = &self.terminal_type {
                    info.emit_title_changed_if_changed(cx);
                }
            }
            TerminalBackendEvent::ColorRequest(index, format) => {
                // It's important that the color request is processed here to retain relative order
                // with other PTY writes. Otherwise applications might witness out-of-order
                // responses to requests. For example: An application sending `OSC 11 ; ? ST`
                // (color request) followed by `CSI c` (request device attributes) would receive
                // the response to `CSI c` first.
                // Instead of locking, we could store the colors in `self.last_content`. But then
                // we might respond with out of date value if a "set color" sequence is immediately
                // followed by a color request sequence.

                let color = self.term.lock().colors()[index]
                    .unwrap_or_else(|| to_vte_rgb(get_color_at_index(index, cx.theme().as_ref())));
                self.write_to_pty(format(color).into_bytes());
            }
            TerminalBackendEvent::ChildExit(exit_status) => {
                self.register_task_finished(Some(exit_status), cx);
            }
        }
    }

    pub fn selection_started(&self) -> bool {
        self.selection_phase == SelectionPhase::Selecting
    }

    fn process_terminal_event(
        &mut self,
        event: &InternalEvent,
        term: &mut AlacrittyTerm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            &InternalEvent::Resize(new_bounds) => {
                let new_bounds = normalize_terminal_bounds(new_bounds);
                trace!("Resizing: new_bounds={new_bounds:?}");

                self.last_content.terminal_bounds = new_bounds;

                if let TerminalType::Pty { pty_tx, .. } = &self.terminal_type {
                    pty_tx.resize(new_bounds);
                }

                resize(term, new_bounds);
                // If there are matches we need to emit a wake up event to
                // invalidate the matches and recalculate their locations
                // in the new terminal layout
                if !self.matches.is_empty() {
                    cx.emit(Event::Wakeup);
                }
            }
            InternalEvent::Clear => {
                trace!("Clearing");
                clear_saved_screen(term);
                cx.emit(Event::Wakeup);
            }
            InternalEvent::Scroll(scroll) => {
                trace!("Scrolling: scroll={scroll:?}");
                scroll_display(term, *scroll);
                self.refresh_hovered_word(window);

                if self.vi_mode_enabled {
                    update_vi_cursor_for_scroll(term, *scroll);
                    if let Some(selection_head) = update_selection_to_vi_cursor(term) {
                        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                        if let Some(selection_text) = selection_text(term) {
                            cx.write_to_primary(ClipboardItem::new_string(selection_text));
                        }

                        self.selection_head = Some(selection_head);
                        cx.emit(Event::SelectionsChanged)
                    }
                }
            }
            InternalEvent::SetSelection(selection) => {
                trace!("Setting selection: selection={selection:?}");
                set_term_selection(term, selection.as_ref());

                #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                if let Some(selection_text) = selection_text(term) {
                    cx.write_to_primary(ClipboardItem::new_string(selection_text));
                }

                if let Some(selection) = selection {
                    self.selection_head = Some(selection.head);
                }
                cx.emit(Event::SelectionsChanged)
            }
            InternalEvent::UpdateSelection(position) => {
                trace!("Updating selection: position={position:?}");
                let (point, side) = grid_point_and_side(
                    *position,
                    self.last_content.terminal_bounds,
                    display_offset(term),
                );

                if update_term_selection(term, point, side) {
                    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                    if let Some(selection_text) = selection_text(term) {
                        cx.write_to_primary(ClipboardItem::new_string(selection_text));
                    }

                    self.selection_head = Some(point);
                    cx.emit(Event::SelectionsChanged)
                }
            }

            InternalEvent::Copy(keep_selection) => {
                trace!("Copying selection: keep_selection={keep_selection:?}");
                if let Some(txt) = selection_text(term) {
                    cx.write_to_clipboard(ClipboardItem::new_string(txt));
                    if !keep_selection.unwrap_or_else(|| {
                        let settings = TerminalSettings::get_global(cx);
                        settings.keep_selection_on_copy
                    }) {
                        self.events.push_back(InternalEvent::SetSelection(None));
                    }
                }
            }
            InternalEvent::ScrollToPoint(point) => {
                trace!("Scrolling to point: point={point:?}");
                scroll_to_point(term, *point);
                self.refresh_hovered_word(window);
            }
            InternalEvent::MoveViCursorToPoint(point) => {
                trace!("Move vi cursor to point: point={point:?}");
                vi_goto_point(term, *point);
                self.refresh_hovered_word(window);
            }
            InternalEvent::ToggleViMode => {
                trace!("Toggling vi mode");
                self.vi_mode_enabled = !self.vi_mode_enabled;
                toggle_term_vi_mode(term);
            }
            InternalEvent::ViMotion(motion) => {
                trace!("Performing vi motion: motion={motion:?}");
                vi_motion(term, *motion);
            }
            InternalEvent::FindHyperlink(position, open) => {
                trace!("Finding hyperlink at position: position={position:?}, open={open:?}");

                let point = grid_point(
                    *position,
                    self.last_content.terminal_bounds,
                    display_offset(term),
                );

                match find_from_terminal_point(
                    term,
                    point,
                    &mut self.hyperlink_regex_searches,
                    self.path_style,
                ) {
                    Some(hyperlink) => {
                        self.process_hyperlink(hyperlink, *open, cx);
                    }
                    None => {
                        self.last_content.last_hovered_word = None;
                        cx.emit(Event::NewNavigationTarget(None));
                    }
                }
            }
            InternalEvent::ProcessHyperlink(hyperlink, open) => {
                self.process_hyperlink(hyperlink.clone(), *open, cx);
            }
        }
    }

    fn process_hyperlink(&mut self, hyperlink: HyperlinkMatch, open: bool, cx: &mut Context<Self>) {
        let HyperlinkMatch {
            text: maybe_url_or_path,
            is_url,
            range,
        } = hyperlink;
        let prev_hovered_word = self.last_content.last_hovered_word.take();

        let target = if is_url {
            if let Some(path) = maybe_url_or_path.strip_prefix("file://") {
                let decoded_path = urlencoding::decode(path)
                    .map(|decoded| decoded.into_owned())
                    .unwrap_or(path.to_owned());

                MaybeNavigationTarget::PathLike(PathLikeTarget {
                    maybe_path: decoded_path,
                    terminal_dir: self.working_directory(),
                })
            } else {
                MaybeNavigationTarget::Url(maybe_url_or_path.clone())
            }
        } else {
            MaybeNavigationTarget::PathLike(PathLikeTarget {
                maybe_path: maybe_url_or_path.clone(),
                terminal_dir: self.working_directory(),
            })
        };

        if open {
            cx.emit(Event::Open(target));
        } else {
            self.update_selected_word(prev_hovered_word, range, maybe_url_or_path, target, cx);
        }
    }

    fn find_hyperlink_at_point(&mut self, point: Point) -> Option<HyperlinkMatch> {
        let term_lock = self.term.lock();
        find_from_terminal_point(
            &term_lock,
            point,
            &mut self.hyperlink_regex_searches,
            self.path_style,
        )
    }

    fn update_selected_word(
        &mut self,
        prev_word: Option<HoveredWord>,
        word_match: Range,
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

    pub fn last_content(&self) -> &Content {
        &self.last_content
    }

    pub fn set_cursor_shape(&mut self, cursor_shape: SettingsCursorShape) {
        set_default_cursor_style(&mut self.term_config, cursor_shape);
        apply_config(&self.term, &self.term_config);
    }

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

        let mut term = self.term.lock();
        self.output_processor.advance(&mut *term, &converted);
        cx.emit(Event::Wakeup);
    }

    pub fn total_lines(&self) -> usize {
        total_lines(&self.term.lock_unfair())
    }

    pub fn viewport_lines(&self) -> usize {
        screen_lines(&self.term.lock_unfair())
    }

    //To test:
    //- Activate match on terminal (scrolling and selection)
    //- Editor search snapping behavior

    pub fn activate_match(&mut self, index: usize) {
        if let Some(search_match) = self.matches.get(index).cloned() {
            self.set_selection(Some(Selection::simple_range(search_match)));
            if self.vi_mode_enabled {
                self.events
                    .push_back(InternalEvent::MoveViCursorToPoint(search_match.end()));
            } else {
                self.events
                    .push_back(InternalEvent::ScrollToPoint(search_match.start()));
            }
        }
    }

    pub fn select_matches(&mut self, matches: &[Range]) {
        let matches_to_select = self
            .matches
            .iter()
            .filter(|self_match| matches.contains(self_match))
            .cloned()
            .collect::<Vec<_>>();
        for match_to_select in matches_to_select {
            self.set_selection(Some(Selection::simple_range(match_to_select)));
        }
    }

    pub fn select_all(&mut self) {
        let term = self.term.lock();
        let range = full_content_range(&term);
        drop(term);
        self.set_selection(Some(Selection::simple_range(range)));
    }

    fn set_selection(&mut self, selection: Option<Selection>) {
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
            .push_back(InternalEvent::Scroll(Scroll::Delta(1)));
    }

    pub fn scroll_up_by(&mut self, lines: usize) {
        self.events
            .push_back(InternalEvent::Scroll(Scroll::Delta(lines as i32)));
    }

    pub fn scroll_line_down(&mut self) {
        self.events
            .push_back(InternalEvent::Scroll(Scroll::Delta(-1)));
    }

    pub fn scroll_down_by(&mut self, lines: usize) {
        self.events
            .push_back(InternalEvent::Scroll(Scroll::Delta(-(lines as i32))));
    }

    pub fn scroll_page_up(&mut self) {
        self.events.push_back(InternalEvent::Scroll(Scroll::PageUp));
    }

    pub fn scroll_page_down(&mut self) {
        self.events
            .push_back(InternalEvent::Scroll(Scroll::PageDown));
    }

    pub fn scroll_to_top(&mut self) {
        self.events.push_back(InternalEvent::Scroll(Scroll::Top));
    }

    pub fn scroll_to_bottom(&mut self) {
        self.events.push_back(InternalEvent::Scroll(Scroll::Bottom));
    }

    pub fn scrolled_to_top(&self) -> bool {
        self.last_content.scrolled_to_top
    }

    pub fn scrolled_to_bottom(&self) -> bool {
        self.last_content.scrolled_to_bottom
    }

    ///Resize the terminal and the PTY.
    pub fn set_size(&mut self, new_bounds: TerminalBounds) {
        let new_bounds = normalize_terminal_bounds(new_bounds);

        let old_bounds = self.last_content.terminal_bounds;
        self.last_content.terminal_bounds = new_bounds;

        // Avoid spamming PTY resizes on pixel-level size changes (e.g. while dragging edges),
        // since those can generate excessive SIGWINCH/reflows and cause visible flicker.
        let requires_resize = old_bounds.num_lines() != new_bounds.num_lines()
            || old_bounds.num_columns() != new_bounds.num_columns()
            || old_bounds.cell_width != new_bounds.cell_width
            || old_bounds.line_height != new_bounds.line_height;

        if !requires_resize {
            return;
        }

        match self.events.back_mut() {
            Some(InternalEvent::Resize(pending_bounds)) => *pending_bounds = new_bounds,
            _ => self.events.push_back(InternalEvent::Resize(new_bounds)),
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
        self.events.push_back(InternalEvent::Scroll(Scroll::Bottom));
        self.events.push_back(InternalEvent::SetSelection(None));

        self.keyboard_input_sent = true;
        let input = input.into();
        #[cfg(any(test, feature = "test-support"))]
        self.input_log.push(input.to_vec());

        self.write_to_pty(input);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn take_input_log(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.input_log)
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
            let cursor_pos = GpuiPoint {
                x: cursor.column as f32 * self.last_content.terminal_bounds.cell_width,
                y: cursor.line as f32 * self.last_content.terminal_bounds.line_height,
            };
            self.events
                .push_back(InternalEvent::UpdateSelection(cursor_pos));
            self.events.push_back(InternalEvent::ViMotion(motion));
            return;
        }

        let scroll_motion = match key.as_ref() {
            "g" => Some(Scroll::Top),
            "G" => Some(Scroll::Bottom),
            "b" if keystroke.modifiers.control => Some(Scroll::PageUp),
            "f" if keystroke.modifiers.control => Some(Scroll::PageDown),
            "d" if keystroke.modifiers.control => {
                let amount = self.last_content.terminal_bounds.line_height().to_f64() as i32 / 2;
                Some(Scroll::Delta(-amount))
            }
            "u" if keystroke.modifiers.control => {
                let amount = self.last_content.terminal_bounds.line_height().to_f64() as i32 / 2;
                Some(Scroll::Delta(amount))
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
                let side = SelectionSide::Right;
                let selection = Selection::new(selection_type, point, side);
                self.events
                    .push_back(InternalEvent::SetSelection(Some(selection)));
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
        let esc = to_esc_str(keystroke, self.last_content.mode, option_as_meta);
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
        let paste_text = if self.last_content.mode.contains(Modes::BRACKETED_PASTE) {
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
            self.process_terminal_event(&e, &mut terminal, window, cx)
        }

        self.last_content = make_content(&terminal, &self.last_content);
    }

    pub fn with_renderable_cells<R>(&self, f: impl for<'a> FnOnce(RenderableCells<'a>) -> R) -> R {
        let term = self.term.lock_unfair();
        let content = term.renderable_content();
        f(RenderableCells::new(content.display_iter))
    }

    pub fn get_content(&self) -> String {
        let term = self.term.lock_unfair();
        content_text(&term)
    }

    pub fn last_n_non_empty_lines(&self, n: usize) -> Vec<String> {
        let terminal = self.term.lock_unfair();
        last_non_empty_lines(&terminal, n)
    }

    pub fn focus_in(&self) {
        if self.last_content.mode.contains(Modes::FOCUS_IN_OUT) {
            self.write_to_pty("\x1b[I".as_bytes());
        }
    }

    pub fn focus_out(&mut self) {
        if self.last_content.mode.contains(Modes::FOCUS_IN_OUT) {
            self.write_to_pty("\x1b[O".as_bytes());
        }
    }

    fn mouse_changed(&mut self, point: Point, side: SelectionSide) -> bool {
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
        self.last_content.mode.intersects(Modes::MOUSE_MODE) && !shift
    }

    pub fn mouse_move(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        let position = e.position - self.last_content.terminal_bounds.bounds.origin;
        if self.mouse_mode(e.modifiers.shift) {
            let (point, side) = grid_point_and_side(
                position,
                self.last_content.terminal_bounds,
                self.last_content.display_offset,
            );

            if self.mouse_changed(point, side) {
                let bytes = mouse_moved_report(
                    point,
                    e.pressed_button,
                    e.modifiers,
                    self.last_content.mode,
                );

                if let Some(bytes) = bytes {
                    self.write_to_pty(bytes);
                }
            }
        } else {
            self.schedule_find_hyperlink(e.modifiers, e.position);
        }
        cx.notify();
    }

    fn schedule_find_hyperlink(&mut self, modifiers: Modifiers, position: GpuiPoint<Pixels>) {
        if self.selection_phase == SelectionPhase::Selecting
            || !modifiers.secondary()
            || !self.last_content.terminal_bounds.bounds.contains(&position)
        {
            self.last_content.last_hovered_word = None;
            return;
        }

        // Throttle hyperlink searches to avoid excessive processing
        let now = Instant::now();
        if self
            .last_hyperlink_search_position
            .map_or(true, |last_pos| {
                // Only search if mouse moved significantly or enough time passed
                let distance_moved = ((position.x - last_pos.x).abs()
                    + (position.y - last_pos.y).abs())
                    > FIND_HYPERLINK_THROTTLE_PX;
                let time_elapsed = now.duration_since(self.last_mouse_move_time).as_millis() > 100;
                distance_moved || time_elapsed
            })
        {
            self.last_mouse_move_time = now;
            self.last_hyperlink_search_position = Some(position);
            self.events.push_back(InternalEvent::FindHyperlink(
                position - self.last_content.terminal_bounds.bounds.origin,
                false,
            ));
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
            .push_back(InternalEvent::SetSelection(Some(selection)));
    }

    pub fn mouse_drag(
        &mut self,
        e: &MouseMoveEvent,
        region: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let position = e.position - self.last_content.terminal_bounds.bounds.origin;
        if !self.mouse_mode(e.modifiers.shift) {
            if let Some(hyperlink) = &self.mouse_down_hyperlink {
                let point = grid_point(
                    position,
                    self.last_content.terminal_bounds,
                    self.last_content.display_offset,
                );

                if !hyperlink.range.contains(point) {
                    self.mouse_down_hyperlink = None;
                } else {
                    return;
                }
            }

            self.selection_phase = SelectionPhase::Selecting;
            // Alacritty has the same ordering, of first updating the selection
            // then scrolling 15ms later
            self.events
                .push_back(InternalEvent::UpdateSelection(position));

            // Doesn't make sense to scroll the alt screen
            if !self.last_content.mode.contains(Modes::ALT_SCREEN) {
                let scroll_lines = match self.drag_line_delta(e, region) {
                    Some(value) => value,
                    None => return,
                };

                self.events
                    .push_back(InternalEvent::Scroll(Scroll::Delta(scroll_lines)));
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

        if e.button == MouseButton::Left
            && e.modifiers.secondary()
            && !self.mouse_mode(e.modifiers.shift)
        {
            self.mouse_down_hyperlink = self.find_hyperlink_at_point(point);

            if self.mouse_down_hyperlink.is_some() {
                return;
            }
        }

        if self.mouse_mode(e.modifiers.shift) {
            let bytes =
                mouse_button_report(point, e.button, e.modifiers, true, self.last_content.mode);

            if let Some(bytes) = bytes {
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

                    if let Some(selection) = selection {
                        self.events
                            .push_back(InternalEvent::SetSelection(Some(selection)));
                    }
                }
                #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                MouseButton::Middle => {
                    if let Some(item) = _cx.read_from_primary() {
                        let text = item.text().unwrap_or_default();
                        self.paste(&text);
                    }
                }
                _ => {}
            }
        }
    }

    pub fn mouse_up(&mut self, e: &MouseUpEvent, cx: &Context<Self>) {
        let setting = TerminalSettings::get_global(cx);

        let position = e.position - self.last_content.terminal_bounds.bounds.origin;
        if self.mouse_mode(e.modifiers.shift) {
            let point = grid_point(
                position,
                self.last_content.terminal_bounds,
                self.last_content.display_offset,
            );

            let bytes =
                mouse_button_report(point, e.button, e.modifiers, false, self.last_content.mode);

            if let Some(bytes) = bytes {
                self.write_to_pty(bytes);
            }
        } else {
            if e.button == MouseButton::Left && setting.copy_on_select {
                self.copy(Some(true));
            }

            if let Some(mouse_down_hyperlink) = self.mouse_down_hyperlink.take() {
                let point = grid_point(
                    position,
                    self.last_content.terminal_bounds,
                    self.last_content.display_offset,
                );

                if let Some(mouse_up_hyperlink) = self.find_hyperlink_at_point(point) {
                    if mouse_down_hyperlink == mouse_up_hyperlink {
                        self.events
                            .push_back(InternalEvent::ProcessHyperlink(mouse_up_hyperlink, true));
                        self.selection_phase = SelectionPhase::Ended;
                        self.last_mouse = None;
                        return;
                    }
                }
            }

            //Hyperlinks
            if self.selection_phase == SelectionPhase::Ended {
                let mouse_cell_index =
                    content_index_for_mouse(position, &self.last_content.terminal_bounds);
                if let Some(link) = self
                    .last_content
                    .cells
                    .get(mouse_cell_index)
                    .and_then(|cell| cell.hyperlink())
                {
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

        if let Some(scroll_lines) = self.determine_scroll_lines(e, scroll_multiplier)
            && scroll_lines != 0
        {
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
                .contains(Modes::ALT_SCREEN | Modes::ALTERNATE_SCROLL)
                && !e.shift
            {
                self.write_to_pty(alt_scroll(scroll_lines));
            } else {
                self.events
                    .push_back(InternalEvent::Scroll(Scroll::Delta(scroll_lines)));
            }
        }
    }

    fn refresh_hovered_word(&mut self, window: &Window) {
        self.schedule_find_hyperlink(window.modifiers(), window.mouse_position());
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

    pub fn find_matches(&self, searcher: Search, cx: &Context<Self>) -> Task<Vec<Range>> {
        let term = self.term.clone();
        cx.background_spawn(async move {
            let term = term.lock();
            search_matches(&term, searcher)
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

    /// Normalizes the command name of the foreground process, if one is known.
    pub fn foreground_process_command_name(&self) -> Option<String> {
        match &self.terminal_type {
            TerminalType::Pty { info, .. } => info
                .current
                .read()
                .as_ref()
                .and_then(|process| foreground_process_command_from_argv(&process.argv)),
            TerminalType::DisplayOnly => None,
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
            TerminalType::Pty { info, .. } => info
                .current
                .read()
                .as_ref()
                .map(|process| process.cwd.clone()),
            TerminalType::DisplayOnly => None,
        }
    }

    pub fn title(&self, truncate: bool) -> String {
        const MAX_CHARS: usize = 25;
        self.title_override
            .as_ref()
            .map(|title_override| title_override.to_string())
            .unwrap_or_else(|| match &self.terminal_type {
                TerminalType::Pty { info, .. } => info
                    .current
                    .read()
                    .as_ref()
                    .map(|fpi| {
                        let process_file = fpi
                            .cwd
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default();

                        let argv = fpi.argv.as_slice();
                        let process_name = format!(
                            "{}{}",
                            fpi.name,
                            if !argv.is_empty() {
                                format!(" {}", (argv[1..]).join(" "))
                            } else {
                                "".to_string()
                            }
                        );
                        let (process_file, process_name) = if truncate {
                            (
                                truncate_and_trailoff(&process_file, MAX_CHARS),
                                truncate_and_trailoff(&process_name, MAX_CHARS),
                            )
                        } else {
                            (process_file, process_name)
                        };
                        format!("{process_file} — {process_name}")
                    })
                    .unwrap_or_else(|| "Terminal".to_string()),
                TerminalType::DisplayOnly => "Terminal".to_string(),
            })
    }

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

    fn register_task_finished(
        &mut self,
        exit_status: Option<ExitStatus>,
        cx: &mut Context<Terminal>,
    ) {
        if let Some(tx) = &self.completion_tx {
            tx.try_send(exit_status).ok();
        }
        if let Some(e) = exit_status {
            self.child_exited = Some(e);
        }
        // For interactive shells, differentiate user-initiated exits from spawn failures.
        let should_close = if self.keyboard_input_sent {
            true
        } else {
            self.child_exited.is_none_or(|e| e.code() == Some(0))
        };
        if should_close {
            cx.emit(Event::CloseTerminal);
        }
    }

    pub fn vi_mode_enabled(&self) -> bool {
        self.vi_mode_enabled
    }

    pub fn clone_builder(&self, cx: &App, cwd: Option<PathBuf>) -> Task<Result<TerminalBuilder>> {
        let working_directory = self.working_directory().or_else(|| cwd);
        TerminalBuilder::new(
            working_directory,
            self.template.shell.clone(),
            self.template.env.clone(),
            self.template.cursor_shape,
            self.template.alternate_scroll,
            self.template.max_scroll_history_lines,
            self.template.path_hyperlink_regexes.clone(),
            self.template.path_hyperlink_timeout_ms,
            self.is_remote_terminal,
            self.template.window_id,
            None,
            cx,
            self.activation_script.clone(),
            self.path_style,
        )
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if let TerminalType::Pty { pty_tx, info } =
            std::mem::replace(&mut self.terminal_type, TerminalType::DisplayOnly)
        {
            pty_tx.shutdown();
            info.terminate_child_process();

            let timer = self.background_executor.timer(Duration::from_millis(100));
            self.background_executor
                .spawn(async move {
                    timer.await;
                    info.kill_child_process();
                })
                .detach();
        }
    }
}

impl EventEmitter<Event> for Terminal {}

fn normalize_path_command_name(command: &str) -> Option<String> {
    const MAX_COMMAND_NAME_LENGTH: usize = 64;

    let command = command.trim();
    if command.is_empty()
        || command.len() > MAX_COMMAND_NAME_LENGTH
        || command.starts_with('.')
        || command.starts_with('-')
        || command.contains('/')
        || command.contains('\\')
    {
        return None;
    }

    let mut command = command.to_ascii_lowercase();
    for suffix in [".exe", ".cmd", ".bat", ".ps1"] {
        if command.ends_with(suffix) {
            command.truncate(command.len() - suffix.len());
            break;
        }
    }

    if command.is_empty()
        || !command.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return None;
    }

    Some(command)
}

fn foreground_process_command_from_argv(argv: &[String]) -> Option<String> {
    let command = argv
        .first()
        .and_then(|command| normalize_path_command_name(command));

    if !matches!(
        command.as_deref(),
        Some("node" | "python" | "python3" | "bun" | "deno")
    ) {
        return command;
    }

    argv.iter()
        .skip(1)
        .filter_map(|argument| normalize_script_command_name(argument))
        .next()
        .or(command)
}

fn normalize_script_command_name(argument: &str) -> Option<String> {
    let path = Path::new(argument);
    let file_stem = path
        .file_stem()
        .and_then(|file_stem| file_stem.to_str())
        .and_then(normalize_path_command_name)?;

    if file_stem != "index" {
        return Some(file_stem);
    }

    path.parent()
        .and_then(|parent| parent.parent())
        .and_then(|package_path| package_path.file_name())
        .and_then(|package_name| package_name.to_str())
        .and_then(|package_name| package_name.strip_suffix("-cli").or(Some(package_name)))
        .and_then(normalize_path_command_name)
}

fn content_index_for_mouse(pos: GpuiPoint<Pixels>, terminal_bounds: &TerminalBounds) -> usize {
    let col = (pos.x / terminal_bounds.cell_width()).round() as usize;
    let clamped_col = min(col, terminal_bounds.num_columns().saturating_sub(1));
    let row = (pos.y / terminal_bounds.line_height()).round() as usize;
    let clamped_row = min(row, terminal_bounds.num_lines().saturating_sub(1));
    clamped_row * terminal_bounds.num_columns() + clamped_col
}

/// Converts an 8 bit ANSI color to its GPUI equivalent.
/// Accepts `usize` for compatibility with the `alacritty::Colors` interface,
/// Other than that use case, should only be called with values in the `[0,255]` range
pub fn get_color_at_index(index: usize, theme: &Theme) -> Hsla {
    let colors = theme.colors();

    match index {
        // 0-15 are the same as the named colors above
        0 => colors.terminal_ansi_black,
        1 => colors.terminal_ansi_red,
        2 => colors.terminal_ansi_green,
        3 => colors.terminal_ansi_yellow,
        4 => colors.terminal_ansi_blue,
        5 => colors.terminal_ansi_magenta,
        6 => colors.terminal_ansi_cyan,
        7 => colors.terminal_ansi_white,
        8 => colors.terminal_ansi_bright_black,
        9 => colors.terminal_ansi_bright_red,
        10 => colors.terminal_ansi_bright_green,
        11 => colors.terminal_ansi_bright_yellow,
        12 => colors.terminal_ansi_bright_blue,
        13 => colors.terminal_ansi_bright_magenta,
        14 => colors.terminal_ansi_bright_cyan,
        15 => colors.terminal_ansi_bright_white,
        // 16-231 are a 6x6x6 RGB color cube, mapped to 0-255 using steps defined by XTerm.
        // See: https://github.com/xterm-x11/xterm-snapshots/blob/master/256colres.pl
        16..=231 => {
            let (r, g, b) = rgb_for_index(index as u8);
            rgba_color(
                if r == 0 { 0 } else { r * 40 + 55 },
                if g == 0 { 0 } else { g * 40 + 55 },
                if b == 0 { 0 } else { b * 40 + 55 },
            )
        }
        // 232-255 are a 24-step grayscale ramp from (8, 8, 8) to (238, 238, 238).
        232..=255 => {
            let i = index as u8 - 232; // Align index to 0..24
            let value = i * 10 + 8;
            rgba_color(value, value, value)
        }
        // For compatibility with the alacritty::Colors interface
        // See: https://github.com/alacritty/alacritty/blob/master/alacritty_terminal/src/term/color.rs
        256 => colors.terminal_foreground,
        257 => colors.terminal_background,
        258 => theme.players().local().cursor,
        259 => colors.terminal_ansi_dim_black,
        260 => colors.terminal_ansi_dim_red,
        261 => colors.terminal_ansi_dim_green,
        262 => colors.terminal_ansi_dim_yellow,
        263 => colors.terminal_ansi_dim_blue,
        264 => colors.terminal_ansi_dim_magenta,
        265 => colors.terminal_ansi_dim_cyan,
        266 => colors.terminal_ansi_dim_white,
        267 => colors.terminal_bright_foreground,
        268 => colors.terminal_ansi_black, // 'Dim Background', non-standard color

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
