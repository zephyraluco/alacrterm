use gpui::{
    AbsoluteLength, AnyElement, App, AvailableSpace, Bounds, ContentMask, Context, DispatchPhase,
    Element, ElementId, Entity, FocusHandle, Font, FontFallbacks, FontFeatures, FontStyle,
    FontWeight, Global, GlobalElementId, HighlightStyle, Hitbox, Hsla, InputHandler,
    InteractiveElement, Interactivity, IntoElement, LayoutId, Length, ModifiersChangedEvent,
    MouseButton, MouseMoveEvent, Pixels, Point as GpuiPoint, StatefulInteractiveElement,
    StrikethroughStyle, Styled, TextRun, TextStyle, UTF16Selection, UnderlineStyle, WhiteSpace,
    Window, div, fill, point, prelude::*, px, relative, size,
};
use itertools::Itertools;
use settings::{IntoGpui, Settings};
use std::time::Instant;
use terminal::{
    Cell, Color, Content, CursorShape, IndexedCell, Modes, NamedColor, Point, Range, Terminal,
    TerminalBounds, is_app_chosen_exact_color as terminal_is_app_chosen_exact_color,
    is_default_background_color, terminal_settings::TerminalSettings,
    ThemeColors
};
use terminal::ActiveColors;
use util::ResultExt;

use std::mem;
use std::{fmt::Debug, rc::Rc};

use crate::{
    BlockContext, BlockProperties, ContentMode, TerminalMode, TerminalView, tooltip::Tooltip,
};

const MIN_FONT_SIZE: Pixels = px(6.0);
const MAX_FONT_SIZE: Pixels = px(100.0);
const BASE_REM_SIZE_IN_PX: f32 = 16.0;
const DEFAULT_UI_FONT_SIZE_IN_PX: f32 = 14.0;

#[derive(Clone)]
struct ThemeFontSettings {
    buffer_font: Font,
    buffer_font_size: Pixels,
}

impl ThemeFontSettings {
    fn buffer_font_size(&self, cx: &App) -> Pixels {
        let font_size = cx
            .try_global::<BufferFontSize>()
            .map(|size| size.0)
            .unwrap_or(self.buffer_font_size);

        clamp_font_size(font_size)
    }
}

#[derive(Default)]
struct BufferFontSize(Pixels);

impl Global for BufferFontSize {}

impl Settings for ThemeFontSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let theme = &content.theme;
        Self {
            buffer_font: Font {
                family: theme.buffer_font_family.as_ref().map_or_else(
                    || "JetBrainsMono Nerd Font".into(),
                    |family| family.0.clone().into(),
                ),
                features: theme
                    .buffer_font_features
                    .clone()
                    .map_or_else(FontFeatures::default, IntoGpui::into_gpui),
                fallbacks: font_fallbacks_from_settings(theme.buffer_font_fallbacks.clone()),
                weight: theme
                    .buffer_font_weight
                    .map_or(FontWeight::NORMAL, IntoGpui::into_gpui),
                style: FontStyle::default(),
            },
            buffer_font_size: theme.buffer_font_size.map_or(px(15.0), IntoGpui::into_gpui),
        }
    }
}

fn font_fallbacks_from_settings(
    fallbacks: Option<Vec<settings::FontFamilyName>>,
) -> Option<FontFallbacks> {
    fallbacks.map(|fallbacks| {
        FontFallbacks::from_fonts(
            fallbacks
                .into_iter()
                .map(|font_family| font_family.0.to_string())
                .collect(),
        )
    })
}

fn clamp_font_size(size: Pixels) -> Pixels {
    size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
}

fn adjusted_font_size(size: Pixels, cx: &App) -> Pixels {
    let font_settings = ThemeFontSettings::get_global(cx);
    let adjusted_font_size = if let Some(BufferFontSize(adjusted_size)) = cx.try_global() {
        let delta = *adjusted_size - font_settings.buffer_font_size;
        size + delta
    } else {
        size
    };

    clamp_font_size(adjusted_font_size)
}

fn ensure_minimum_contrast(fg: Hsla, _bg: Hsla, _minimum_contrast: f32) -> Hsla {
    fg
}

#[derive(Clone, Copy)]
enum EditorCursorShape {
    Block,
    Hollow,
    Underline,
    Bar,
}

struct CursorLayout {
    bounds: Bounds<Pixels>,
    color: Hsla,
    shape: EditorCursorShape,
}

impl CursorLayout {
    fn new<T>(
        origin: GpuiPoint<Pixels>,
        width: Pixels,
        height: Pixels,
        color: Hsla,
        shape: EditorCursorShape,
        _text: Option<T>,
    ) -> Self {
        Self {
            bounds: Bounds::new(origin, size(width, height)),
            color,
            shape,
        }
    }

    fn paint(&mut self, origin: GpuiPoint<Pixels>, window: &mut Window, _cx: &mut App) {
        let mut bounds = self.bounds;
        bounds.origin += origin;
        match self.shape {
            EditorCursorShape::Block | EditorCursorShape::Hollow => {
                window.paint_quad(fill(bounds, self.color));
            }
            EditorCursorShape::Underline => {
                let underline = Bounds::new(
                    point(
                        bounds.origin.x,
                        bounds.origin.y + bounds.size.height - px(2.),
                    ),
                    size(bounds.size.width, px(2.)),
                );
                window.paint_quad(fill(underline, self.color));
            }
            EditorCursorShape::Bar => {
                let bar = Bounds::new(bounds.origin, size(px(2.), bounds.size.height));
                window.paint_quad(fill(bar, self.color));
            }
        }
    }
}

struct HighlightedRangeLine {
    start_x: Pixels,
    end_x: Pixels,
}

struct HighlightedRange {
    start_y: Pixels,
    line_height: Pixels,
    lines: Vec<HighlightedRangeLine>,
    color: Hsla,
    corner_radius: Pixels,
}

impl HighlightedRange {
    fn paint(&self, _is_active: bool, _bounds: Bounds<Pixels>, window: &mut Window) {
        for (ix, line) in self.lines.iter().enumerate() {
            let bounds = Bounds::new(
                point(line.start_x, self.start_y + ix as f32 * self.line_height),
                size(line.end_x - line.start_x, self.line_height),
            );
            let mut quad = fill(bounds, self.color);
            quad.corner_radii = self.corner_radius.into();
            window.paint_quad(quad);
        }
    }
}

/// The information generated during layout that is necessary for painting.
pub struct LayoutState {
    hitbox: Hitbox,
    batched_text_runs: Vec<BatchedTextRun>,
    rects: Vec<LayoutRect>,
    relative_highlighted_ranges: Vec<(Range, Hsla)>,
    cursor: Option<CursorLayout>,
    ime_cursor_bounds: Option<Bounds<Pixels>>,
    background_color: Hsla,
    dimensions: TerminalBounds,
    mode: Modes,
    display_offset: usize,
    hyperlink_tooltip: Option<AnyElement>,
    block_below_cursor_element: Option<AnyElement>,
    base_text_style: TextStyle,
    content_mode: ContentMode,
}

/// Helper struct for converting terminal cursor points to displayed cursor points.
#[derive(Copy, Clone)]
struct DisplayCursor {
    line: i32,
    col: usize,
}

impl DisplayCursor {
    fn from(cursor_point: Point, display_offset: usize) -> Self {
        Self {
            line: cursor_point.line + display_offset as i32,
            col: cursor_point.column,
        }
    }

    pub fn line(&self) -> i32 {
        self.line
    }

    pub fn col(&self) -> usize {
        self.col
    }
}

#[derive(Copy, Clone, Debug, Default)]
pub struct LayoutPoint {
    line: i32,
    column: i32,
}

impl LayoutPoint {
    fn new(line: i32, column: i32) -> Self {
        Self { line, column }
    }

    pub fn line(&self) -> i32 {
        self.line
    }

    pub fn column(&self) -> i32 {
        self.column
    }
}

/// A batched text run that combines multiple adjacent cells with the same style
#[derive(Debug)]
pub struct BatchedTextRun {
    pub start_point: LayoutPoint,
    pub text: String,
    pub cell_count: usize,
    pub style: TextRun,
    pub font_size: AbsoluteLength,
}

impl BatchedTextRun {
    fn new_from_char(
        start_point: LayoutPoint,
        c: char,
        style: TextRun,
        font_size: AbsoluteLength,
    ) -> Self {
        let mut text = String::with_capacity(100); // Pre-allocate for typical line length
        text.push(c);
        BatchedTextRun {
            start_point,
            text,
            cell_count: 1,
            style,
            font_size,
        }
    }

    fn can_append(&self, other_style: &TextRun) -> bool {
        self.style.font == other_style.font
            && self.style.color == other_style.color
            && self.style.background_color == other_style.background_color
            && self.style.underline == other_style.underline
            && self.style.strikethrough == other_style.strikethrough
    }

    fn append_char(&mut self, c: char) {
        self.append_char_internal(c, true);
    }

    fn append_zero_width_chars(&mut self, chars: &[char]) {
        for &c in chars {
            self.append_char_internal(c, false);
        }
    }

    fn append_char_internal(&mut self, c: char, counts_cell: bool) {
        self.text.push(c);
        if counts_cell {
            self.cell_count += 1;
        }
        self.style.len += c.len_utf8();
    }

    pub fn paint(
        &self,
        origin: GpuiPoint<Pixels>,
        dimensions: &TerminalBounds,
        window: &mut Window,
        cx: &mut App,
    ) {
        let pos = GpuiPoint::new(
            origin.x + self.start_point.column as f32 * dimensions.cell_width,
            origin.y + self.start_point.line as f32 * dimensions.line_height,
        );

        window
            .text_system()
            .shape_line(
                self.text.clone().into(),
                self.font_size.to_pixels(window.rem_size()),
                std::slice::from_ref(&self.style),
                Some(dimensions.cell_width),
            )
            .paint(
                pos,
                dimensions.line_height,
                gpui::TextAlign::Left,
                None,
                window,
                cx,
            )
            .log_err();
    }
}

#[derive(Clone, Debug, Default)]
pub struct LayoutRect {
    point: LayoutPoint,
    num_of_cells: usize,
    color: Hsla,
}

impl LayoutRect {
    fn new(point: LayoutPoint, num_of_cells: usize, color: Hsla) -> LayoutRect {
        LayoutRect {
            point,
            num_of_cells,
            color,
        }
    }

    pub fn paint(
        &self,
        origin: GpuiPoint<Pixels>,
        dimensions: &TerminalBounds,
        window: &mut Window,
    ) {
        let position = {
            let layout_point = self.point;
            point(
                (origin.x + layout_point.column as f32 * dimensions.cell_width).floor(),
                origin.y + layout_point.line as f32 * dimensions.line_height,
            )
        };
        let size = point(
            (dimensions.cell_width * self.num_of_cells as f32).ceil(),
            dimensions.line_height,
        )
        .into();

        window.paint_quad(fill(Bounds::new(position, size), self.color));
    }
}

/// Represents a rectangular region with a specific background color
#[derive(Debug, Clone)]
struct BackgroundRegion {
    start_line: i32,
    start_col: i32,
    end_line: i32,
    end_col: i32,
    color: Hsla,
}

impl BackgroundRegion {
    fn new(line: i32, col: i32, color: Hsla) -> Self {
        BackgroundRegion {
            start_line: line,
            start_col: col,
            end_line: line,
            end_col: col,
            color,
        }
    }

    /// Check if this region can be merged with another region
    fn can_merge_with(&self, other: &BackgroundRegion) -> bool {
        if self.color != other.color {
            return false;
        }

        // Check if regions are adjacent horizontally
        if self.start_line == other.start_line && self.end_line == other.end_line {
            return self.end_col + 1 == other.start_col || other.end_col + 1 == self.start_col;
        }

        // Check if regions are adjacent vertically with same column span
        if self.start_col == other.start_col && self.end_col == other.end_col {
            return self.end_line + 1 == other.start_line || other.end_line + 1 == self.start_line;
        }

        false
    }

    /// Merge this region with another region
    fn merge_with(&mut self, other: &BackgroundRegion) {
        self.start_line = self.start_line.min(other.start_line);
        self.start_col = self.start_col.min(other.start_col);
        self.end_line = self.end_line.max(other.end_line);
        self.end_col = self.end_col.max(other.end_col);
    }
}

pub trait TerminalLayoutCell {
    fn point(&self) -> Point;
    fn cell(&self) -> &Cell;
}

impl TerminalLayoutCell for IndexedCell {
    fn point(&self) -> Point {
        self.point
    }

    fn cell(&self) -> &Cell {
        &self.cell
    }
}

impl TerminalLayoutCell for &IndexedCell {
    fn point(&self) -> Point {
        self.point
    }

    fn cell(&self) -> &Cell {
        &self.cell
    }
}

/// Merge background regions to minimize the number of rectangles
fn merge_background_regions(regions: Vec<BackgroundRegion>) -> Vec<BackgroundRegion> {
    if regions.is_empty() {
        return regions;
    }

    let mut merged = regions;
    let mut changed = true;

    // Keep merging until no more merges are possible
    while changed {
        changed = false;
        let mut i = 0;

        while i < merged.len() {
            let mut j = i + 1;
            while j < merged.len() {
                if merged[i].can_merge_with(&merged[j]) {
                    let other = merged.remove(j);
                    merged[i].merge_with(&other);
                    changed = true;
                } else {
                    j += 1;
                }
            }
            i += 1;
        }
    }

    merged
}

/// The GPUI element that paints the terminal.
/// We need to keep a reference to the model for mouse events, do we need it for any other terminal stuff, or can we move that to connection?
pub struct TerminalElement {
    terminal: Entity<Terminal>,
    terminal_view: Entity<TerminalView>,
    focus: FocusHandle,
    focused: bool,
    cursor_visible: bool,
    interactivity: Interactivity,
    mode: TerminalMode,
    block_below_cursor: Option<Rc<BlockProperties>>,
}

impl InteractiveElement for TerminalElement {
    fn interactivity(&mut self) -> &mut Interactivity {
        &mut self.interactivity
    }
}

impl StatefulInteractiveElement for TerminalElement {}

impl TerminalElement {
    pub fn new(
        terminal: Entity<Terminal>,
        terminal_view: Entity<TerminalView>,
        focus: FocusHandle,
        focused: bool,
        cursor_visible: bool,
        block_below_cursor: Option<Rc<BlockProperties>>,
        mode: TerminalMode,
    ) -> TerminalElement {
        TerminalElement {
            terminal,
            terminal_view,
            focused,
            focus: focus.clone(),
            cursor_visible,
            block_below_cursor,
            mode,
            interactivity: Default::default(),
        }
        .track_focus(&focus)
    }

    pub fn layout_grid<T: TerminalLayoutCell>(
        grid: impl Iterator<Item = T>,
        start_line_offset: i32,
        text_style: &TextStyle,
        hyperlink: Option<(HighlightStyle, &Range)>,
        minimum_contrast: f32,
        cx: &App,
    ) -> (Vec<LayoutRect>, Vec<BatchedTextRun>) {
        let start_time = Instant::now();
        let colors = cx.terminal_colors();

        // Pre-allocate with estimated capacity to reduce reallocations
        let estimated_cells = grid.size_hint().0;
        let estimated_runs = estimated_cells / 10; // Estimate ~10 cells per run
        let estimated_regions = estimated_cells / 20; // Estimate ~20 cells per background region

        let mut batched_runs = Vec::with_capacity(estimated_runs);
        let mut cell_count = 0;

        // Collect background regions for efficient merging
        let mut background_regions: Vec<BackgroundRegion> = Vec::with_capacity(estimated_regions);
        let mut current_batch: Option<BatchedTextRun> = None;

        // First pass: collect all cells and their backgrounds
        let linegroups = grid.into_iter().chunk_by(|cell| cell.point().line);
        for (line_index, (_, line)) in linegroups.into_iter().enumerate() {
            let display_line = start_line_offset + line_index as i32;

            // Flush any existing batch at line boundaries
            if let Some(batch) = current_batch.take() {
                batched_runs.push(batch);
            }

            let mut previous_cell_had_extras = false;

            for cell in line {
                let point = cell.point();
                let cell = cell.cell();
                let mut fg = cell.foreground();
                let mut bg = cell.background();
                if cell.is_inverse() {
                    mem::swap(&mut fg, &mut bg);
                }

                // Collect background regions (skip default background)
                if !is_default_background_color(bg) {
                    let color = convert_color(&bg, colors);
                    let col = point.column as i32;

                    // Try to extend the last region if it's on the same line with the same color
                    if let Some(last_region) = background_regions.last_mut()
                        && last_region.color == color
                        && last_region.start_line == display_line
                        && last_region.end_line == display_line
                        && last_region.end_col + 1 == col
                    {
                        last_region.end_col = col;
                    } else {
                        background_regions.push(BackgroundRegion::new(display_line, col, color));
                    }
                }
                // Skip wide character spacers - they're just placeholders for the second cell of wide characters
                if cell.is_wide_char_spacer() {
                    continue;
                }

                // Skip spaces that follow cells with extras (emoji variation sequences)
                if cell.character() == ' ' && previous_cell_had_extras {
                    previous_cell_had_extras = false;
                    continue;
                }
                // Update tracking for next iteration
                previous_cell_had_extras =
                    matches!(cell.zerowidth(), Some(chars) if !chars.is_empty());

                //Layout current cell text
                {
                    if !is_blank(cell) {
                        cell_count += 1;
                        let cell_style = TerminalElement::cell_style(
                            point,
                            cell,
                            fg,
                            bg,
                            colors,
                            text_style,
                            hyperlink,
                            minimum_contrast,
                        );

                        let cell_point = LayoutPoint::new(display_line, point.column as i32);
                        let zero_width_chars = cell.zerowidth();

                        // Try to batch with existing run
                        if let Some(ref mut batch) = current_batch {
                            if batch.can_append(&cell_style)
                                && batch.start_point.line == cell_point.line
                                && batch.start_point.column + batch.cell_count as i32
                                    == cell_point.column
                            {
                                batch.append_char(cell.character());
                                if let Some(chars) = zero_width_chars {
                                    batch.append_zero_width_chars(chars);
                                }
                            } else {
                                // Flush current batch and start new one
                                let old_batch = current_batch.take().unwrap();
                                batched_runs.push(old_batch);
                                let mut new_batch = BatchedTextRun::new_from_char(
                                    cell_point,
                                    cell.character(),
                                    cell_style,
                                    text_style.font_size,
                                );
                                if let Some(chars) = zero_width_chars {
                                    new_batch.append_zero_width_chars(chars);
                                }
                                current_batch = Some(new_batch);
                            }
                        } else {
                            // Start new batch
                            let mut new_batch = BatchedTextRun::new_from_char(
                                cell_point,
                                cell.character(),
                                cell_style,
                                text_style.font_size,
                            );
                            if let Some(chars) = zero_width_chars {
                                new_batch.append_zero_width_chars(chars);
                            }
                            current_batch = Some(new_batch);
                        }
                    };
                }
            }
        }

        // Flush any remaining batch
        if let Some(batch) = current_batch {
            batched_runs.push(batch);
        }

        // Second pass: merge background regions and convert to layout rects
        let region_count = background_regions.len();
        let merged_regions = merge_background_regions(background_regions);
        let mut rects = Vec::with_capacity(merged_regions.len() * 2); // Estimate 2 rects per merged region

        // Convert merged regions to layout rects
        // Since LayoutRect only supports single-line rectangles, we need to split multi-line regions
        for region in merged_regions {
            for line in region.start_line..=region.end_line {
                rects.push(LayoutRect::new(
                    LayoutPoint::new(line, region.start_col),
                    (region.end_col - region.start_col + 1) as usize,
                    region.color,
                ));
            }
        }

        let layout_time = start_time.elapsed();

        log::debug!(
            "Terminal layout_grid: {} cells processed, \
            {} batched runs created, {} rects (from {} merged regions), \
            layout took {:?}",
            cell_count,
            batched_runs.len(),
            rects.len(),
            region_count,
            layout_time
        );

        (rects, batched_runs)
    }

    /// Computes the cursor position based on the cursor point and terminal dimensions.
    fn cursor_position(
        cursor_point: DisplayCursor,
        size: TerminalBounds,
    ) -> Option<GpuiPoint<Pixels>> {
        if cursor_point.line() < size.num_lines() as i32 {
            // When on pixel boundaries round the origin down
            Some(point(
                (cursor_point.col() as f32 * size.cell_width()).floor(),
                (cursor_point.line() as f32 * size.line_height()).floor(),
            ))
        } else {
            None
        }
    }

    /// Checks if a character is a decorative block/box-like character that should
    /// preserve its exact colors without contrast adjustment.
    ///
    /// This specifically targets characters used as visual connectors, separators,
    /// and borders where color matching with adjacent backgrounds is critical.
    /// Regular icons (git, folders, etc.) are excluded as they need to remain readable.
    ///
    /// Fixes https://github.com/zed-industries/zed/issues/34234
    fn is_decorative_character(ch: char) -> bool {
        matches!(
            ch as u32,
            // Unicode Box Drawing and Block Elements
            0x2500..=0x257F // Box Drawing (└ ┐ ─ │ etc.)
            | 0x2580..=0x259F // Block Elements (▀ ▄ █ ░ ▒ ▓ etc.)
            | 0x25A0..=0x25FF // Geometric Shapes (■ ▶ ● etc. - includes triangular/circular separators)

            // Private Use Area - Powerline separator symbols only
            | 0xE0B0..=0xE0B7 // Powerline separators: triangles (E0B0-E0B3) and half circles (E0B4-E0B7)
            | 0xE0B8..=0xE0BF // Powerline separators: corner triangles
            | 0xE0C0..=0xE0CA // Powerline separators: flames (E0C0-E0C3), pixelated (E0C4-E0C7), and ice (E0C8 & E0CA)
            | 0xE0CC..=0xE0D1 // Powerline separators: honeycombs (E0CC-E0CD) and lego (E0CE-E0D1)
            | 0xE0D2..=0xE0D7 // Powerline separators: trapezoid (E0D2 & E0D4) and inverted triangles (E0D6-E0D7)
        )
    }

    /// Whether the application explicitly picked this foreground color and does not
    /// want it adjusted for contrast: 24-bit true color (`\e[38;2;R;G;Bm`) or a
    /// specific entry in the 256-color palette (`\e[38;5;Nm`) where N >= 16 (the
    /// 6x6x6 cube at 16..=231 and the 24-step grayscale ramp at 232..=255).
    /// Indices 0..=15 still go through contrast adjustment since those map to
    /// theme-defined ANSI colors that can clash with the theme background.
    fn is_app_chosen_exact_color(fg: &Color) -> bool {
        terminal_is_app_chosen_exact_color(*fg)
    }

    /// Converts terminal cell styles to GPUI text styles and background color.
    fn cell_style(
        point: Point,
        cell: &Cell,
        fg: Color,
        bg: Color,
        colors: &ThemeColors,
        text_style: &TextStyle,
        hyperlink: Option<(HighlightStyle, &Range)>,
        minimum_contrast: f32,
    ) -> TextRun {
        let skip_contrast = Self::is_app_chosen_exact_color(&fg);
        let mut fg = convert_color(&fg, colors);
        let bg = convert_color(&bg, colors);

        if !skip_contrast && !Self::is_decorative_character(cell.character()) {
            fg = ensure_minimum_contrast(fg, bg, minimum_contrast);
        }

        // Use a dim multiplier that stays close to the existing Alacritty look.
        if cell.is_dim() {
            fg.a *= 0.7;
        }

        let underline =
            (cell.has_underline() || cell.hyperlink().is_some()).then(|| UnderlineStyle {
                color: Some(fg),
                thickness: Pixels::from(1.0),
                wavy: cell.has_undercurl(),
            });

        let strikethrough = cell.has_strikeout().then(|| StrikethroughStyle {
            color: Some(fg),
            thickness: Pixels::from(1.0),
        });

        let weight = if cell.is_bold() {
            FontWeight::BOLD
        } else {
            text_style.font_weight
        };

        let style = if cell.is_italic() {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        };

        let mut result = TextRun {
            len: cell.character().len_utf8(),
            color: fg,
            background_color: None,
            font: Font {
                weight,
                style,
                ..text_style.font()
            },
            underline,
            strikethrough,
        };

        if let Some((style, range)) = hyperlink
            && range.contains(point)
        {
            if let Some(underline) = style.underline {
                result.underline = Some(underline);
            }

            if let Some(color) = style.color {
                result.color = color;
            }
        }

        result
    }

    fn generic_button_handler<E>(
        connection: Entity<Terminal>,
        focus_handle: FocusHandle,
        steal_focus: bool,
        f: impl Fn(&mut Terminal, &E, &mut Context<Terminal>),
    ) -> impl Fn(&E, &mut Window, &mut App) {
        move |event, window, cx| {
            if steal_focus {
                window.focus(&focus_handle, cx);
            } else if !focus_handle.is_focused(window) {
                return;
            }
            connection.update(cx, |terminal, cx| {
                f(terminal, event, cx);

                cx.notify();
            })
        }
    }

    fn register_mouse_listeners(
        &mut self,
        mode: Modes,
        hitbox: &Hitbox,
        content_mode: &ContentMode,
        window: &mut Window,
    ) {
        let focus = self.focus.clone();
        let terminal = self.terminal.clone();
        let terminal_view = self.terminal_view.clone();

        self.interactivity.on_mouse_down(MouseButton::Left, {
            let terminal = terminal.clone();
            let focus = focus.clone();
            let terminal_view = terminal_view.clone();

            move |e, window, cx| {
                window.focus(&focus, cx);

                let scroll_top = terminal_view.read(cx).scroll_top;
                terminal.update(cx, |terminal, cx| {
                    let mut adjusted_event = e.clone();
                    if scroll_top > Pixels::ZERO {
                        adjusted_event.position.y += scroll_top;
                    }
                    terminal.mouse_down(&adjusted_event, cx);
                    cx.notify();
                })
            }
        });

        window.on_mouse_event({
            let terminal = self.terminal.clone();
            let hitbox = hitbox.clone();
            let focus = focus.clone();
            let terminal_view = terminal_view;
            move |e: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }

                if e.pressed_button.is_some() && !cx.has_active_drag() && focus.is_focused(window) {
                    let hovered = hitbox.is_hovered(window);

                    let scroll_top = terminal_view.read(cx).scroll_top;
                    terminal.update(cx, |terminal, cx| {
                        if terminal.selection_started() || hovered {
                            let mut adjusted_event = e.clone();
                            if scroll_top > Pixels::ZERO {
                                adjusted_event.position.y += scroll_top;
                            }
                            terminal.mouse_drag(&adjusted_event, hitbox.bounds, cx);
                            cx.notify();
                        }
                    })
                }

                if hitbox.is_hovered(window) {
                    terminal.update(cx, |terminal, cx| {
                        terminal.mouse_move(e, cx);
                    })
                }
            }
        });

        self.interactivity.on_mouse_up(
            MouseButton::Left,
            TerminalElement::generic_button_handler(
                terminal.clone(),
                focus.clone(),
                false,
                move |terminal, e, cx| {
                    terminal.mouse_up(e, cx);
                },
            ),
        );
        self.interactivity.on_mouse_down(
            MouseButton::Middle,
            TerminalElement::generic_button_handler(
                terminal.clone(),
                focus.clone(),
                true,
                move |terminal, e, cx| {
                    terminal.mouse_down(e, cx);
                },
            ),
        );

        if content_mode.is_scrollable() {
            self.interactivity.on_scroll_wheel({
                let terminal_view = self.terminal_view.downgrade();
                move |e, window, cx| {
                    terminal_view
                        .update(cx, |terminal_view, cx| {
                            if matches!(terminal_view.mode, TerminalMode::Standalone)
                                || terminal_view.focus_handle.is_focused(window)
                            {
                                terminal_view.scroll_wheel(e, cx);
                                cx.notify();
                            }
                        })
                        .ok();
                }
            });
        }

        // Mouse mode handlers:
        // All mouse modes need the extra click handlers
        if mode.intersects(Modes::MOUSE_MODE) {
            self.interactivity.on_mouse_down(
                MouseButton::Right,
                TerminalElement::generic_button_handler(
                    terminal.clone(),
                    focus.clone(),
                    true,
                    move |terminal, e, cx| {
                        terminal.mouse_down(e, cx);
                    },
                ),
            );
            self.interactivity.on_mouse_up(
                MouseButton::Right,
                TerminalElement::generic_button_handler(
                    terminal.clone(),
                    focus.clone(),
                    false,
                    move |terminal, e, cx| {
                        terminal.mouse_up(e, cx);
                    },
                ),
            );
            self.interactivity.on_mouse_up(
                MouseButton::Middle,
                TerminalElement::generic_button_handler(
                    terminal,
                    focus,
                    false,
                    move |terminal, e, cx| {
                        terminal.mouse_up(e, cx);
                    },
                ),
            );
        }
    }

    fn rem_size(&self, cx: &mut App) -> Option<Pixels> {
        let buffer_font_size = ThemeFontSettings::get_global(cx).buffer_font_size(cx);
        let default_font_size_scale = DEFAULT_UI_FONT_SIZE_IN_PX / BASE_REM_SIZE_IN_PX;
        let default_font_size_delta = 1.0 - default_font_size_scale;

        Some(buffer_font_size * (1.0 + default_font_size_delta))
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = LayoutState;

    fn id(&self) -> Option<ElementId> {
        self.interactivity.element_id.clone()
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let height: Length = match self.terminal_view.read(cx).content_mode(window, cx) {
            ContentMode::Inline {
                displayed_lines,
                total_lines: _,
            } => {
                let rem_size = self.rem_size(cx).unwrap_or_else(|| window.rem_size());
                let line_height = f32::from(window.text_style().font_size.to_pixels(rem_size))
                    * TerminalSettings::get_global(cx).line_height.value();
                px(displayed_lines as f32 * line_height).into()
            }
            ContentMode::Scrollable => {
                if let TerminalMode::Embedded { .. } = &self.mode {
                    let term = self.terminal.read(cx);
                    if !term.scrolled_to_top() && !term.scrolled_to_bottom() && self.focused {
                        self.interactivity.occlude_mouse();
                    }
                }

                relative(1.).into()
            }
        };

        let layout_id = self.interactivity.request_layout(
            global_id,
            inspector_id,
            window,
            cx,
            |mut style, window, cx| {
                style.size.width = relative(1.).into();
                style.size.height = height;

                window.request_layout(style, None, cx)
            },
        );
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let rem_size = self.rem_size(cx);
        self.interactivity.prepaint(
            global_id,
            inspector_id,
            bounds,
            bounds.size,
            window,
            cx,
            |_, _, hitbox, window, cx| {
                let hitbox = hitbox.unwrap();
                let theme_font_settings = ThemeFontSettings::get_global(cx);
                let terminal_settings = TerminalSettings::get_global(cx);
                let minimum_contrast = terminal_settings.minimum_contrast;

                let font_family = terminal_settings.font_family.as_ref().map_or_else(
                    || theme_font_settings.buffer_font.family.clone(),
                    |font_family| font_family.0.clone().into(),
                );

                let font_fallbacks = terminal_settings
                    .font_fallbacks
                    .clone()
                    .or_else(|| theme_font_settings.buffer_font.fallbacks.clone());

                let font_features = terminal_settings
                    .font_features
                    .as_ref()
                    .unwrap_or(&FontFeatures::disable_ligatures())
                    .clone();

                let font_weight = terminal_settings.font_weight.unwrap_or_default();

                let line_height = terminal_settings.line_height.value();

                let font_size = match &self.mode {
                    TerminalMode::Embedded { .. } => {
                        window.text_style().font_size.to_pixels(window.rem_size())
                    }
                    TerminalMode::Standalone => terminal_settings.font_size.map_or_else(
                        || theme_font_settings.buffer_font_size(cx),
                        |size| adjusted_font_size(size, cx),
                    ),
                };

                let colors = cx.terminal_colors().clone();
                let link_color = colors.terminal_ansi_blue;

                let link_style = HighlightStyle {
                    color: Some(link_color),
                    font_weight: Some(font_weight),
                    font_style: None,
                    background_color: None,
                    underline: Some(UnderlineStyle {
                        thickness: px(1.0),
                        color: Some(link_color),
                        wavy: false,
                    }),
                    strikethrough: None,
                    fade_out: None,
                };

                let text_style = TextStyle {
                    font_family,
                    font_features,
                    font_weight,
                    font_fallbacks,
                    font_size: font_size.into(),
                    font_style: FontStyle::Normal,
                    line_height: px(line_height).into(),
                    background_color: Some(colors.terminal_ansi_background),
                    white_space: WhiteSpace::Normal,
                    // These are going to be overridden per-cell
                    color: colors.terminal_foreground,
                    ..Default::default()
                };

                let text_system = cx.text_system();
                let match_color = colors.terminal_foreground;
                let gutter;
                let (dimensions, line_height_px) = {
                    let rem_size = window.rem_size();
                    let font_pixels = text_style.font_size.to_pixels(rem_size);
                    let line_height = f32::from(font_pixels) * line_height;
                    let font_id = cx.text_system().resolve_font(&text_style.font());

                    let cell_width = text_system
                        .advance(font_id, font_pixels, 'm')
                        .unwrap()
                        .width;
                    gutter = cell_width;

                    let mut size = bounds.size;
                    size.width -= gutter;
                    let available_height = size.height;

                    // https://github.com/zed-industries/zed/issues/2750
                    // if the terminal is one column wide, rendering 🦀
                    // causes alacritty to misbehave.
                    if size.width < cell_width * 2.0 {
                        size.width = cell_width * 2.0;
                    }

                    let mut origin = bounds.origin;
                    origin.x += gutter;

                    if matches!(self.terminal_view.read(cx).mode, TerminalMode::Standalone) {
                        let scale_factor = window.scale_factor();
                        let line_height_pixels = px(line_height);
                        let line_height_device_px = (f32::from(line_height_pixels) * scale_factor)
                            .round()
                            .max(1.0) as i32;
                        let available_height_device_px =
                            (f32::from(available_height) * scale_factor)
                                .floor()
                                .max(0.0) as i32;

                        let rows =
                            ((available_height_device_px / line_height_device_px) as usize).max(1);
                        let snapped_height_device_px = (rows as i32) * line_height_device_px;
                        let padding_device_px =
                            (available_height_device_px - snapped_height_device_px).max(0);

                        let snapped_height =
                            px(snapped_height_device_px as f32 / scale_factor.max(1.0));
                        let padding = px(padding_device_px as f32 / scale_factor.max(1.0));

                        size.height = snapped_height;
                        if self.terminal.read(cx).scrolled_to_bottom() {
                            origin.y += padding;
                        }
                    }

                    // Snap to device pixels to avoid subpixel jitter while resizing.
                    // Terminal rendering is grid-based; allowing fractional origins can cause the
                    // glyph rasterization to shift between frames, which looks like flicker.
                    let scale_factor = window.scale_factor();
                    let snap_px = |value: Pixels| {
                        Pixels::from((f32::from(value) * scale_factor).floor() / scale_factor)
                    };
                    origin.x = snap_px(origin.x);
                    origin.y = snap_px(origin.y);

                    (
                        TerminalBounds::new(px(line_height), cell_width, Bounds { origin, size }),
                        line_height,
                    )
                };

                let search_matches = self.terminal.read(cx).matches.clone();

                let background_color = colors.terminal_background;

                let (last_hovered_word, hover_tooltip) =
                    self.terminal.update(cx, |terminal, cx| {
                        terminal.set_size(dimensions);
                        terminal.sync(window, cx);

                        if window.modifiers().secondary()
                            && bounds.contains(&window.mouse_position())
                            && self.terminal_view.read(cx).hover.is_some()
                        {
                            let registered_hover = self.terminal_view.read(cx).hover.as_ref();
                            if terminal.last_content.last_hovered_word.as_ref()
                                == registered_hover.map(|hover| &hover.hovered_word)
                            {
                                (
                                    terminal.last_content.last_hovered_word.clone(),
                                    registered_hover.map(|hover| hover.tooltip.clone()),
                                )
                            } else {
                                (None, None)
                            }
                        } else {
                            (None, None)
                        }
                    });

                let scroll_top = self.terminal_view.read(cx).scroll_top;
                let hyperlink_tooltip = hover_tooltip.map(|hover_tooltip| {
                    let offset = dimensions.bounds.origin - point(px(0.), scroll_top);
                    let mut element = div()
                        .size_full()
                        .id("terminal-element")
                        .tooltip(Tooltip::text(hover_tooltip))
                        .into_any_element();
                    element.prepaint_as_root(offset, bounds.size.into(), window, cx);
                    element
                });

                let Content {
                    cells,
                    mode,
                    display_offset,
                    cursor_char,
                    selection,
                    cursor,
                    ..
                } = &self.terminal.read(cx).last_content;
                let mode = *mode;
                let display_offset = *display_offset;

                // searches, highlights to a single range representations
                let mut relative_highlighted_ranges = Vec::new();
                for search_match in search_matches {
                    relative_highlighted_ranges.push((search_match, match_color))
                }
                if let Some(selection) = selection {
                    relative_highlighted_ranges.push((
                        selection.point_range(),
                        colors.terminal_ansi_blue,
                    ));
                }

                // then have that representation be converted to the appropriate highlight data structure

                let content_mode = self.terminal_view.read(cx).content_mode(window, cx);

                // Calculate the intersection of the terminal's bounds with the current
                // content mask (the visible viewport after all parent clipping).
                // This allows us to only render cells that are actually visible, which is
                // critical for performance when terminals are inside scrollable containers
                // like the Agent Panel thread view.
                //
                // This optimization is analogous to the editor optimization in PR #45077
                // which fixed performance issues with large AutoHeight editors inside Lists.
                let content_bounds = dimensions.bounds;
                let visible_bounds = window.content_mask().bounds;
                let intersection = visible_bounds.intersect(&content_bounds);

                // If the terminal is entirely outside the viewport, skip all cell processing.
                // This handles the case where the terminal has been scrolled past (above or
                // below the viewport), similar to the editor fix in PR #45077 where start_row
                // could exceed max_row when the editor was positioned above the viewport.
                let (rects, batched_text_runs) = if intersection.size.height <= px(0.)
                    || intersection.size.width <= px(0.)
                {
                    (Vec::new(), Vec::new())
                } else if intersection == content_bounds {
                    // Fast path: terminal fully visible, no clipping needed.
                    // Avoid grouping/allocation overhead by streaming cells directly.
                    TerminalElement::layout_grid(
                        cells.iter(),
                        0,
                        &text_style,
                        last_hovered_word
                            .as_ref()
                            .map(|last_hovered_word| (link_style, &last_hovered_word.word_match)),
                        minimum_contrast,
                        cx,
                    )
                } else {
                    // Calculate which screen rows are visible based on pixel positions.
                    // This works for both Scrollable and Inline modes because we filter
                    // by screen position (enumerated line group index), not by the cell's
                    // internal line number (which can be negative in Scrollable mode for
                    // scrollback history).
                    let rows_above_viewport = f32::from(
                        (intersection.top() - content_bounds.top()).max(px(0.)) / line_height_px,
                    ) as usize;
                    let visible_row_count =
                        f32::from((intersection.size.height / line_height_px).ceil()) as usize + 1;

                    TerminalElement::layout_grid(
                        // Group cells by line and filter to only the visible screen rows.
                        // skip() and take() work on enumerated line groups (screen position),
                        // making this work regardless of the actual cell.point.line values.
                        cells
                            .iter()
                            .chunk_by(|c| c.point.line)
                            .into_iter()
                            .skip(rows_above_viewport)
                            .take(visible_row_count)
                            .flat_map(|(_, line_cells)| line_cells),
                        rows_above_viewport as i32,
                        &text_style,
                        last_hovered_word
                            .as_ref()
                            .map(|last_hovered_word| (link_style, &last_hovered_word.word_match)),
                        minimum_contrast,
                        cx,
                    )
                };

                // Layout cursor. Rectangle is used for IME, so we should lay it out even
                // if we don't end up showing it.
                let cursor_point = DisplayCursor::from(cursor.point, display_offset);
                let cursor_text = {
                    let str_trxt = cursor_char.to_string();
                    let len = str_trxt.len();
                    window.text_system().shape_line(
                        str_trxt.into(),
                        text_style.font_size.to_pixels(window.rem_size()),
                        &[TextRun {
                            len,
                            font: text_style.font(),
                            color: colors.terminal_ansi_background,
                            ..Default::default()
                        }],
                        None,
                    )
                };

                // For whitespace, use cell width to avoid cursor stretching.
                // For other characters, use the larger of shaped width and cell width
                // to properly cover wide characters like emojis.
                let cursor_width = if cursor_char.is_whitespace() {
                    dimensions.cell_width()
                } else {
                    cursor_text.width.max(dimensions.cell_width())
                };

                let ime_cursor_bounds = TerminalElement::cursor_position(cursor_point, dimensions)
                    .map(|cursor_position| Bounds {
                        origin: cursor_position,
                        size: size(cursor_width.ceil(), dimensions.line_height),
                    });

                let cursor = if let CursorShape::Hidden = cursor.shape {
                    None
                } else {
                    let focused = self.focused;
                    ime_cursor_bounds.map(move |bounds| {
                        let (shape, text) = match cursor.shape {
                            CursorShape::Block if !focused => (EditorCursorShape::Hollow, None),
                            CursorShape::Block => (EditorCursorShape::Block, Some(cursor_text)),
                            CursorShape::Underline if !focused => (EditorCursorShape::Hollow, None),
                            CursorShape::Underline => (EditorCursorShape::Underline, None),
                            CursorShape::Bar if !focused => (EditorCursorShape::Hollow, None),
                            CursorShape::Bar => (EditorCursorShape::Bar, None),
                            CursorShape::HollowBlock => (EditorCursorShape::Hollow, None),
                            CursorShape::Hidden => unreachable!(),
                        };

                        CursorLayout::new(
                            bounds.origin,
                            bounds.size.width,
                            bounds.size.height,
                            colors.terminal_ansi_bright_blue,
                            shape,
                            text,
                        )
                    })
                };

                let block_below_cursor_element = if let Some(block) = &self.block_below_cursor {
                    let terminal = self.terminal.read(cx);
                    if terminal.last_content.display_offset == 0 {
                        let target_line = terminal.last_content.cursor.point.line + 1;
                        let render = &block.render;
                        let mut block_cx = BlockContext {
                            window,
                            context: cx,
                            dimensions,
                        };
                        let element = render(&mut block_cx);
                        let mut element = div().occlude().child(element).into_any_element();
                        let available_space = size(
                            AvailableSpace::Definite(dimensions.width() + gutter),
                            AvailableSpace::Definite(
                                block.height as f32 * dimensions.line_height(),
                            ),
                        );
                        let origin = GpuiPoint::new(bounds.origin.x, dimensions.bounds.origin.y)
                            + point(px(0.), target_line as f32 * dimensions.line_height())
                            - point(px(0.), scroll_top);
                        window.with_rem_size(rem_size, |window| {
                            element.prepaint_as_root(origin, available_space, window, cx);
                        });
                        Some(element)
                    } else {
                        None
                    }
                } else {
                    None
                };

                LayoutState {
                    hitbox,
                    batched_text_runs,
                    cursor,
                    ime_cursor_bounds,
                    background_color,
                    dimensions,
                    rects,
                    relative_highlighted_ranges,
                    mode,
                    display_offset,
                    hyperlink_tooltip,
                    block_below_cursor_element,
                    base_text_style: text_style,
                    content_mode,
                }
            },
        )
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        layout: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let paint_start = Instant::now();
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            let scroll_top = self.terminal_view.read(cx).scroll_top;

            window.paint_quad(fill(bounds, layout.background_color));
            let origin = layout.dimensions.bounds.origin - GpuiPoint::new(px(0.), scroll_top);
            let scale_factor = window.scale_factor();
            let snap_px = |value: Pixels| {
                Pixels::from((f32::from(value) * scale_factor).floor() / scale_factor)
            };
            let origin = point(snap_px(origin.x), snap_px(origin.y));

            let marked_text_cloned: Option<String> = {
                let ime_state = &self.terminal_view.read(cx).ime_state;
                ime_state.as_ref().map(|state| state.marked_text.clone())
            };

            let terminal_input_handler = TerminalInputHandler {
                terminal: self.terminal.clone(),
                terminal_view: self.terminal_view.clone(),
                cursor_bounds: layout.ime_cursor_bounds.map(|bounds| bounds + origin),
            };

            self.register_mouse_listeners(
                layout.mode,
                &layout.hitbox,
                &layout.content_mode,
                window,
            );
            if window.modifiers().secondary()
                && bounds.contains(&window.mouse_position())
                && self.terminal_view.read(cx).hover.is_some()
            {
                window.set_cursor_style(gpui::CursorStyle::PointingHand, &layout.hitbox);
            } else {
                window.set_cursor_style(gpui::CursorStyle::IBeam, &layout.hitbox);
            }

            let original_cursor = layout.cursor.take();
            let hyperlink_tooltip = layout.hyperlink_tooltip.take();
            let block_below_cursor_element = layout.block_below_cursor_element.take();
            self.interactivity.paint(
                global_id,
                inspector_id,
                bounds,
                Some(&layout.hitbox),
                window,
                cx,
                |_, window, cx| {
                    window.handle_input(&self.focus, terminal_input_handler, cx);

                    window.on_key_event({
                        let this = self.terminal.clone();
                        move |event: &ModifiersChangedEvent, phase, window, cx| {
                            if phase != DispatchPhase::Bubble {
                                return;
                            }

                            this.update(cx, |term, cx| {
                                term.try_modifiers_change(&event.modifiers, window, cx)
                            });
                        }
                    });

                    for rect in &layout.rects {
                        rect.paint(origin, &layout.dimensions, window);
                    }

                    for (relative_highlighted_range, color) in &layout.relative_highlighted_ranges {
                        if let Some((start_y, highlighted_range_lines)) =
                            to_highlighted_range_lines(relative_highlighted_range, layout, origin)
                        {
                            let corner_radius = 0.15 * layout.dimensions.line_height;
                            let hr = HighlightedRange {
                                start_y,
                                line_height: layout.dimensions.line_height,
                                lines: highlighted_range_lines,
                                color: *color,
                                corner_radius: corner_radius,
                            };
                            hr.paint(true, bounds, window);
                        }
                    }

                    // Paint batched text runs instead of individual cells
                    let text_paint_start = Instant::now();
                    for batch in &layout.batched_text_runs {
                        batch.paint(origin, &layout.dimensions, window, cx);
                    }
                    let text_paint_time = text_paint_start.elapsed();

                    if let Some(text_to_mark) = &marked_text_cloned
                        && !text_to_mark.is_empty()
                        && let Some(ime_bounds) = layout.ime_cursor_bounds
                    {
                        let ime_position = (ime_bounds + origin).origin;
                        let mut ime_style = layout.base_text_style.clone();
                        ime_style.underline = Some(UnderlineStyle {
                            color: Some(ime_style.color),
                            thickness: px(1.0),
                            wavy: false,
                        });

                        let shaped_line = window.text_system().shape_line(
                            text_to_mark.clone().into(),
                            ime_style.font_size.to_pixels(window.rem_size()),
                            &[TextRun {
                                len: text_to_mark.len(),
                                font: ime_style.font(),
                                color: ime_style.color,
                                underline: ime_style.underline,
                                ..Default::default()
                            }],
                            None,
                        );

                        // Paint background to cover terminal text behind marked text
                        let ime_background_bounds = Bounds::new(
                            ime_position,
                            size(shaped_line.width, layout.dimensions.line_height),
                        );
                        window.paint_quad(fill(ime_background_bounds, layout.background_color));

                        shaped_line
                            .paint(
                                ime_position,
                                layout.dimensions.line_height,
                                gpui::TextAlign::Left,
                                None,
                                window,
                                cx,
                            )
                            .log_err();
                    }

                    if self.cursor_visible
                        && marked_text_cloned.is_none()
                        && let Some(mut cursor) = original_cursor
                    {
                        cursor.paint(origin, window, cx);
                    }

                    if let Some(mut element) = block_below_cursor_element {
                        element.paint(window, cx);
                    }

                    if let Some(mut element) = hyperlink_tooltip {
                        element.paint(window, cx);
                    }

                    log::debug!(
                        "Terminal paint: {} text runs, {} rects, \
                        text paint took {:?}, total paint took {total_paint_time:?}",
                        layout.batched_text_runs.len(),
                        layout.rects.len(),
                        text_paint_time,
                        total_paint_time = paint_start.elapsed()
                    );
                },
            );
        });
    }
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

struct TerminalInputHandler {
    terminal: Entity<Terminal>,
    terminal_view: Entity<TerminalView>,
    cursor_bounds: Option<Bounds<Pixels>>,
}

impl InputHandler for TerminalInputHandler {
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<UTF16Selection> {
        if self
            .terminal
            .read(cx)
            .last_content
            .mode
            .contains(Modes::ALT_SCREEN)
        {
            None
        } else {
            Some(UTF16Selection {
                range: 0..0,
                reversed: false,
            })
        }
    }

    fn marked_text_range(
        &mut self,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<std::ops::Range<usize>> {
        self.terminal_view.read(cx).marked_text_range()
    }

    fn text_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: &mut Option<std::ops::Range<usize>>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<String> {
        None
    }

    fn replace_text_in_range(
        &mut self,
        _replacement_range: Option<std::ops::Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.terminal_view.update(cx, |view, view_cx| {
            view.clear_marked_text(view_cx);
            view.commit_text(text, view_cx);
        });
        window.invalidate_character_coordinates();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<std::ops::Range<usize>>,
        new_text: &str,
        _new_marked_range: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        cx: &mut App,
    ) {
        self.terminal_view.update(cx, |view, view_cx| {
            view.set_marked_text(new_text.to_string(), view_cx);
        });
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut App) {
        self.terminal_view.update(cx, |view, view_cx| {
            view.clear_marked_text(view_cx);
        });
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: std::ops::Range<usize>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        let term_bounds = self.terminal_view.read(cx).terminal_bounds(cx);

        let mut bounds = self.cursor_bounds?;
        let offset_x = term_bounds.cell_width * range_utf16.start as f32;
        bounds.origin.x += offset_x;

        Some(bounds)
    }

    fn apple_press_and_hold_enabled(&mut self) -> bool {
        false
    }

    fn character_index_for_point(
        &mut self,
        _point: GpuiPoint<Pixels>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<usize> {
        None
    }
}

pub fn is_blank(cell: &Cell) -> bool {
    if cell.character() != ' ' {
        return false;
    }

    if !is_default_background_color(cell.background()) {
        return false;
    }

    if cell.hyperlink().is_some() {
        return false;
    }

    if cell.has_visible_style_modifier() {
        return false;
    }

    true
}

fn to_highlighted_range_lines(
    range: &Range,
    layout: &LayoutState,
    origin: GpuiPoint<Pixels>,
) -> Option<(Pixels, Vec<HighlightedRangeLine>)> {
    // Step 1. Normalize the points to be viewport relative.
    // When display_offset = 1, here's how the grid is arranged:
    //-2,0 -2,1...
    //--- Viewport top
    //-1,0 -1,1...
    //--------- Terminal Top
    // 0,0  0,1...
    // 1,0  1,1...
    //--- Viewport Bottom
    // 2,0  2,1...
    //--------- Terminal Bottom

    // Normalize to viewport relative, from terminal relative.
    // lines are i32s, which are negative above the top left corner of the terminal
    // If the user has scrolled, we use the display_offset to tell us which offset
    // of the grid data we should be looking at. But for the rendering step, we don't
    // want negatives. We want things relative to the 'viewport' (the area of the grid
    // which is currently shown according to the display offset)
    let display_offset = i32::try_from(layout.display_offset).unwrap_or(i32::MAX);
    let unclamped_start_line = range.start().line.saturating_add(display_offset);
    let unclamped_start_column = range.start().column;
    let unclamped_end_line = range.end().line.saturating_add(display_offset);
    let unclamped_end_column = range.end().column;

    // Step 2. Clamp range to viewport, and return None if it doesn't overlap
    if unclamped_end_line < 0 || unclamped_start_line > layout.dimensions.num_lines() as i32 {
        return None;
    }

    let clamped_start_line = unclamped_start_line.max(0) as usize;

    let clamped_end_line = unclamped_end_line.min(layout.dimensions.num_lines() as i32) as usize;

    // Convert the start of the range to pixels
    let start_y = origin.y + clamped_start_line as f32 * layout.dimensions.line_height;

    // Step 3. Expand ranges that cross lines into a collection of single-line ranges.
    //  (also convert to pixels)
    let mut highlighted_range_lines = Vec::new();
    for line in clamped_start_line..=clamped_end_line {
        let mut line_start = 0;
        let mut line_end = layout.dimensions.num_columns();

        if line == clamped_start_line && unclamped_start_line >= 0 {
            line_start = unclamped_start_column;
        }
        if line == clamped_end_line && unclamped_end_line <= layout.dimensions.num_lines() as i32 {
            line_end = unclamped_end_column + 1; // +1 for inclusive
        }

        highlighted_range_lines.push(HighlightedRangeLine {
            start_x: origin.x + line_start as f32 * layout.dimensions.cell_width,
            end_x: origin.x + line_end as f32 * layout.dimensions.cell_width,
        });
    }

    Some((start_y, highlighted_range_lines))
}

/// Converts a 2, 8, or 24 bit color ANSI color to the GPUI equivalent.
pub fn convert_color(fg: &Color, colors: &ThemeColors) -> Hsla {

    match fg {
        // Named and theme defined colors
        Color::Named(color) => match color {
            NamedColor::Black => colors.terminal_ansi_black,
            NamedColor::Red => colors.terminal_ansi_red,
            NamedColor::Green => colors.terminal_ansi_green,
            NamedColor::Yellow => colors.terminal_ansi_yellow,
            NamedColor::Blue => colors.terminal_ansi_blue,
            NamedColor::Magenta => colors.terminal_ansi_magenta,
            NamedColor::Cyan => colors.terminal_ansi_cyan,
            NamedColor::White => colors.terminal_ansi_white,
            NamedColor::BrightBlack => colors.terminal_ansi_bright_black,
            NamedColor::BrightRed => colors.terminal_ansi_bright_red,
            NamedColor::BrightGreen => colors.terminal_ansi_bright_green,
            NamedColor::BrightYellow => colors.terminal_ansi_bright_yellow,
            NamedColor::BrightBlue => colors.terminal_ansi_bright_blue,
            NamedColor::BrightMagenta => colors.terminal_ansi_bright_magenta,
            NamedColor::BrightCyan => colors.terminal_ansi_bright_cyan,
            NamedColor::BrightWhite => colors.terminal_ansi_bright_white,
            NamedColor::Foreground => colors.terminal_foreground,
            NamedColor::Background => colors.terminal_ansi_background,
            NamedColor::Cursor => colors.terminal_ansi_blue,
            NamedColor::DimBlack => colors.terminal_ansi_dim_black,
            NamedColor::DimRed => colors.terminal_ansi_dim_red,
            NamedColor::DimGreen => colors.terminal_ansi_dim_green,
            NamedColor::DimYellow => colors.terminal_ansi_dim_yellow,
            NamedColor::DimBlue => colors.terminal_ansi_dim_blue,
            NamedColor::DimMagenta => colors.terminal_ansi_dim_magenta,
            NamedColor::DimCyan => colors.terminal_ansi_dim_cyan,
            NamedColor::DimWhite => colors.terminal_ansi_dim_white,
            NamedColor::BrightForeground => colors.terminal_bright_foreground,
            NamedColor::DimForeground => colors.terminal_dim_foreground,
        },
        // 'True' colors
        Color::Spec(rgb) => terminal::rgba_color(rgb.r, rgb.g, rgb.b),
        // 8 bit, indexed colors
        Color::Indexed(i) => terminal::get_color_at_index(*i as usize, &ThemeColors::dark()),
    }
}
