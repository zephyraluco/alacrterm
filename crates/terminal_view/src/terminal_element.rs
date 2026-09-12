//! 高性能终端渲染核心。
//!
//! 从 Zed 的 `crates/terminal_view/src/terminal_element.rs` 移植并精简而来：
//!
//! **保留的渲染功能**
//! - `layout_grid`：cell 网格 → 批量文本 run / 背景矩形 / 块字符矩形
//! - `BatchedTextRun`：连续同风格 cell 合并为一次整形、一次绘制
//! - `merge_background_regions`：背景矩形合并（减少 GPU quad 提交）
//! - 块字符 subcell 网格（8 列 × 24 行）：`█▀▄▌`、sextant、quadrant、shade、Powerline
//! - APCA 最小对比度调整（`ensure_minimum_contrast`）
//! - 光标渲染（Block / Underline / Bar / Hollow）与 IME 组合文本
//! - 选择 / 搜索高亮、视口裁剪（content_mask）、设备像素对齐
//!
//! **删除的 Zed 依赖**
//! - `editor`（CursorLayout / HighlightedRange / BlinkManager）
//! - `ui` / `theme` / `theme_settings` / `settings` / `workspace` / `project`
//!
//! **替代实现**
//! - 光标与高亮直接用 `window.paint_quad` 绘制
//! - APCA 对比度移植到 `crate::contrast`（仅依赖 gpui `Hsla`）
//! - 主题色使用 `terminal::TerminalColors`（本地定义）

use std::mem;
use std::ops::Range as StdRange;
use std::sync::Arc;

use gpui::{
    AbsoluteLength, App, Bounds, ContentMask, Context, DispatchPhase, Element, ElementId, Entity,
    FocusHandle, Font, FontFeatures, FontStyle, FontWeight, GlobalElementId, HighlightStyle, Hitbox,
    Hsla, InputHandler, InspectorElementId, InteractiveElement, Interactivity, IntoElement, LayoutId,
    Length, ModifiersChangedEvent, MouseButton, MouseMoveEvent, Pixels, Point as GpuiPoint,
    ShapedLine, SharedString, StatefulInteractiveElement, StrikethroughStyle, TextRun, TextStyle,
    UTF16Selection, UnderlineStyle, WhiteSpace, Window, fill, hsla, point, px, relative, size,
};
use itertools::Itertools;
use terminal::{
    Cell, Color, CursorShape, IndexedCell, Modes, NamedColor, Point, Range, Terminal, TerminalBounds,
    TerminalColors, get_color_at_index, is_app_chosen_exact_color, is_default_background_color,
    rgba_color,
};

use crate::{TerminalView, contrast::ensure_minimum_contrast};

/// 终端渲染设置（本地定义，替代 Zed 的 `TerminalSettings` + `ThemeSettings`）。
#[derive(Clone)]
pub struct TerminalRenderSettings {
    pub font_family: SharedString,
    pub font_size: Pixels,
    pub font_weight: FontWeight,
    /// 行高 = 字号 × 该系数
    pub line_height_multiplier: f32,
    /// APCA 最小对比度（0 表示关闭）
    pub minimum_contrast: f32,
    pub cursor_shape: CursorShape,
    /// 聚焦时光标是否闪烁
    pub cursor_blinks: bool,
    pub colors: TerminalColors,
    /// 选择高亮色（半透明）
    pub selection_color: Hsla,
    /// 搜索匹配高亮色（半透明）
    pub search_match_color: Hsla,
    /// 超链接颜色
    pub link_color: Hsla,
}

impl Default for TerminalRenderSettings {
    fn default() -> Self {
        Self {
            font_family: "JetBrainsMono Nerd Font".into(),
            font_size: px(15.0),
            font_weight: FontWeight::default(),
            line_height_multiplier: 1.3,
            minimum_contrast: 45.0,
            cursor_shape: CursorShape::Block,
            cursor_blinks: true,
            colors: TerminalColors::dark(),
            selection_color: hsla(0.6, 0.5, 0.55, 0.45),
            search_match_color: hsla(0.12, 0.9, 0.5, 0.35),
            link_color: hsla(0.58, 0.9, 0.62, 1.0),
        }
    }
}

/// The information generated during layout that is necessary for painting.
pub struct LayoutState {
    hitbox: Hitbox,
    batched_text_runs: Vec<BatchedTextRun>,
    block_element_rects: Vec<BlockElementLayoutRect>,
    rects: Vec<LayoutRect>,
    relative_highlighted_ranges: Vec<(Range, Hsla)>,
    cursor: Option<CursorLayout>,
    ime_cursor_bounds: Option<Bounds<Pixels>>,
    background_color: Hsla,
    dimensions: TerminalBounds,
    mode: Modes,
    display_offset: usize,
    base_text_style: TextStyle,
}

/// 光标形状（本地定义，替代 `editor::CursorLayout` 的光标类型）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum CursorKind {
    Block,
    Underline,
    Bar,
    Hollow,
}

/// 光标绘制信息（本地实现，替代 Zed 的 `editor::CursorLayout`）。
struct CursorLayout {
    origin: GpuiPoint<Pixels>,
    width: Pixels,
    height: Pixels,
    color: Hsla,
    kind: CursorKind,
    /// Block 光标内显示的字符（整形后的行）
    text: Option<ShapedLine>,
}

impl CursorLayout {
    fn paint(&self, origin: GpuiPoint<Pixels>, window: &mut Window, cx: &mut App) {
        let bounds = Bounds::new(origin + self.origin, size(self.width, self.height));
        match self.kind {
            CursorKind::Block => {
                window.paint_quad(fill(bounds, self.color));
                if let Some(text) = &self.text {
                    if let Err(error) = text.paint(
                        bounds.origin,
                        self.height,
                        gpui::TextAlign::Left,
                        None,
                        window,
                        cx,
                    ) {
                        log::error!("failed to paint cursor text: {error}");
                    }
                }
            }
            CursorKind::Underline => {
                let thickness = px(2.0);
                let y = bounds.origin.y + bounds.size.height - thickness;
                window.paint_quad(fill(
                    Bounds::new(
                        point(bounds.origin.x, y),
                        size(bounds.size.width, thickness),
                    ),
                    self.color,
                ));
            }
            CursorKind::Bar => {
                let width = px(2.0);
                window.paint_quad(fill(
                    Bounds::new(bounds.origin, size(width, bounds.size.height)),
                    self.color,
                ));
            }
            CursorKind::Hollow => {
                // 空心框：绘制四条边
                let t = px(1.0);
                let o = bounds.origin;
                let s = bounds.size;
                window.paint_quad(fill(Bounds::new(o, size(s.width, t)), self.color));
                window.paint_quad(fill(
                    Bounds::new(point(o.x, o.y + s.height - t), size(s.width, t)),
                    self.color,
                ));
                window.paint_quad(fill(Bounds::new(o, size(t, s.height)), self.color));
                window.paint_quad(fill(
                    Bounds::new(point(o.x + s.width - t, o.y), size(t, s.height)),
                    self.color,
                ));
            }
        }
    }
}

/// 高亮行的一个像素段（本地实现，替代 `editor::HighlightedRangeLine`）。
struct HighlightedRangeLine {
    start_x: Pixels,
    end_x: Pixels,
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
}

#[derive(Copy, Clone, Debug, Default)]
struct LayoutPoint {
    line: i32,
    column: i32,
}

impl LayoutPoint {
    fn new(line: i32, column: i32) -> Self {
        Self { line, column }
    }
}

/// A batched text run that combines multiple adjacent cells with the same style
#[derive(Debug)]
struct BatchedTextRun {
    start_point: LayoutPoint,
    text: String,
    cell_count: usize,
    style: TextRun,
    font_size: AbsoluteLength,
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

        if let Err(error) = window
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
        {
            log::error!("failed to paint text run: {error}");
        }
    }
}

/// Block element glyphs are painted on a subcell grid: each terminal cell is
/// divided into 8 columns (for eighth blocks) and 24 lines (LCM of the 8-way
/// splits of eighth blocks and the 3-way splits of sextants).
const BLOCK_SUBCELL_COLUMNS: i32 = 8;
const BLOCK_SUBCELL_LINES: i32 = 24;

#[derive(Clone, Debug)]
struct BlockElementLayoutRect {
    point: LayoutPoint,
    num_of_columns: usize,
    num_of_lines: usize,
    color: Hsla,
}

impl BlockElementLayoutRect {
    fn new(point: LayoutPoint, num_of_columns: usize, num_of_lines: usize, color: Hsla) -> Self {
        Self {
            point,
            num_of_columns,
            num_of_lines,
            color,
        }
    }

    pub fn paint(
        &self,
        origin: GpuiPoint<Pixels>,
        dimensions: &TerminalBounds,
        window: &mut Window,
    ) {
        let subcell_width = dimensions.cell_width / BLOCK_SUBCELL_COLUMNS as f32;
        let subcell_height = dimensions.line_height / BLOCK_SUBCELL_LINES as f32;
        let position = point(
            origin.x + self.point.column as f32 * subcell_width,
            origin.y + self.point.line as f32 * subcell_height,
        );
        let size = size(
            subcell_width * self.num_of_columns as f32,
            subcell_height * self.num_of_lines as f32,
        );

        window.paint_quad(fill(Bounds::new(position, size), self.color));
    }
}

#[derive(Clone, Debug, Default)]
struct LayoutRect {
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

/// Represents a rectangular region with a specific color on a logical grid.
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

    fn with_extents(
        start_line: i32,
        start_col: i32,
        end_line: i32,
        end_col: i32,
        color: Hsla,
    ) -> Self {
        BackgroundRegion {
            start_line,
            start_col,
            end_line,
            end_col,
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

/// Merge grid regions to minimize the number of rectangles.
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
pub struct TerminalElement {
    terminal: Entity<Terminal>,
    terminal_view: Entity<TerminalView>,
    focus: FocusHandle,
    focused: bool,
    cursor_visible: bool,
    interactivity: Interactivity,
    settings: Arc<TerminalRenderSettings>,
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
        settings: Arc<TerminalRenderSettings>,
    ) -> TerminalElement {
        TerminalElement {
            terminal,
            terminal_view,
            focused,
            focus: focus.clone(),
            cursor_visible,
            interactivity: Default::default(),
            settings,
        }
        .track_focus(&focus)
    }

    /// 把网格 cell 转换为三类图元：背景矩形、批量文本 run、块字符矩形。
    fn layout_grid<'a>(
        grid: impl Iterator<Item = &'a IndexedCell>,
        start_line_offset: i32,
        text_style: &TextStyle,
        hyperlink: Option<(HighlightStyle, &Range)>,
        minimum_contrast: f32,
        colors: &TerminalColors,
        cx: &App,
    ) -> (
        Vec<LayoutRect>,
        Vec<BatchedTextRun>,
        Vec<BlockElementLayoutRect>,
    ) {
        // Pre-allocate with estimated capacity to reduce reallocations
        let estimated_cells = grid.size_hint().0;
        let estimated_runs = estimated_cells / 10; // Estimate ~10 cells per run
        let estimated_regions = estimated_cells / 20; // Estimate ~20 cells per background region

        let mut batched_runs = Vec::with_capacity(estimated_runs);
        let mut block_element_regions = Vec::new();
        let mut cell_count = 0;

        // Collect background regions for efficient merging
        let mut background_regions: Vec<BackgroundRegion> = Vec::with_capacity(estimated_regions);
        let mut current_batch: Option<BatchedTextRun> = None;

        // First pass: collect all cells and their backgrounds
        let linegroups = grid.chunk_by(|cell| cell.point.line);
        for (line_index, (_, line)) in linegroups.into_iter().enumerate() {
            let display_line = start_line_offset + line_index as i32;

            // Flush any existing batch at line boundaries
            if let Some(batch) = current_batch.take() {
                batched_runs.push(batch);
            }

            let mut previous_cell_had_extras = false;

            for ic in line {
                let point = ic.point;
                let cell = &ic.cell;
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
                        if Self::collect_block_element_regions(
                            cell_point,
                            cell.character(),
                            cell_style.color,
                            &mut block_element_regions,
                        ) {
                            if let Some(batch) = current_batch.take() {
                                batched_runs.push(batch);
                            }
                            continue;
                        }

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

        let block_element_rects = Self::block_element_regions_to_rects(block_element_regions);

        let _ = cell_count;
        let _ = cx;

        (rects, batched_runs, block_element_rects)
    }

    /// Computes the cursor position based on the cursor point and terminal dimensions.
    fn cursor_position(
        cursor_point: DisplayCursor,
        size: TerminalBounds,
    ) -> Option<GpuiPoint<Pixels>> {
        if cursor_point.line < size.num_lines() as i32 {
            // When on pixel boundaries round the origin down
            Some(point(
                (cursor_point.col as f32 * size.cell_width()).floor(),
                (cursor_point.line as f32 * size.line_height()).floor(),
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
    fn is_decorative_character(ch: char) -> bool {
        matches!(
            ch as u32,
            // Unicode Box Drawing and Block Elements
            0x2500..=0x257F // Box Drawing (└ ┐ ─ │ etc.)
            | 0x2580..=0x259F // Block Elements (▀ ▄ █ ░ ▒ ▓ etc.)
            | 0x25A0..=0x25FF // Geometric Shapes (■ ▶ ● etc.)
            | 0x1FB00..=0x1FB3B // Symbols for Legacy Computing sextants used by terminal QR renderers

            // Private Use Area - Powerline separator symbols only
            | 0xE0B0..=0xE0B7 // Powerline separators: triangles and half circles
            | 0xE0B8..=0xE0BF // Powerline separators: corner triangles
            | 0xE0C0..=0xE0CA // Powerline separators: flames, pixelated, ice
            | 0xE0CC..=0xE0D1 // Powerline separators: honeycombs and lego
            | 0xE0D2..=0xE0D7 // Powerline separators: trapezoid and inverted triangles
        )
    }

    /// Returns the filled subcells of a sextant character as a bitmap, where
    /// bit `row * 2 + column` is set when that 2x3 subcell is filled.
    ///
    /// U+1FB00..=U+1FB3B enumerate all 2x3 fill combinations except the four
    /// that already exist as Block Elements (empty, `▌` = 0b010101,
    /// `▐` = 0b101010, and `█` = 0b111111), hence the gap adjustments.
    fn sextant_char_to_filled_bits(ch: char) -> Option<u8> {
        let offset = (ch as u32).checked_sub(0x1FB00)?;
        if offset > 0x3B {
            return None;
        }

        Some((offset + 1 + u32::from(offset >= 20) + u32::from(offset >= 40)) as u8)
    }

    /// Returns the filled quadrants of a quadrant character as a bitmap, where
    /// bit `row * 2 + column` is set when that 2x2 subcell is filled.
    fn quadrant_char_to_filled_bits(ch: char) -> Option<u8> {
        Some(match ch {
            '▘' => 0b0001,
            '▝' => 0b0010,
            '▖' => 0b0100,
            '▗' => 0b1000,
            '▚' => 0b1001,
            '▞' => 0b0110,
            '▛' => 0b0111,
            '▜' => 0b1011,
            '▙' => 0b1101,
            '▟' => 0b1110,
            _ => return None,
        })
    }

    /// Returns `(column, line, num_of_columns, num_of_lines)` in subcell units
    /// for block element characters that consist of a single rectangle.
    fn block_char_to_rect(ch: char) -> Option<(i32, i32, i32, i32)> {
        let codepoint = ch as u32;
        Some(match codepoint {
            // ▀ upper half
            0x2580 => (0, 0, 8, 12),
            // ▁▂▃▄▅▆▇█ lower blocks of 1..=8 eighths
            0x2581..=0x2588 => {
                let eighths = (codepoint - 0x2580) as i32;
                (0, 24 - eighths * 3, 8, eighths * 3)
            }
            // ▉▊▋▌▍▎▏ left blocks of 7..=1 eighths
            0x2589..=0x258F => (0, 0, (0x2590 - codepoint) as i32, 24),
            // ▐ right half
            0x2590 => (4, 0, 4, 24),
            // ▔ upper eighth
            0x2594 => (0, 0, 8, 3),
            // ▕ right eighth
            0x2595 => (7, 0, 1, 24),
            _ => return None,
        })
    }

    /// Approximates the shade characters `░▒▓` with the foreground color at
    /// reduced opacity instead of the stipple patterns fonts use, trading
    /// pattern fidelity for seamless cell coverage.
    fn shade_char_to_opacity(ch: char) -> Option<f32> {
        match ch {
            '░' => Some(0.25),
            '▒' => Some(0.5),
            '▓' => Some(0.75),
            _ => None,
        }
    }

    fn collect_block_element_regions(
        point: LayoutPoint,
        ch: char,
        color: Hsla,
        regions: &mut Vec<BackgroundRegion>,
    ) -> bool {
        if let Some((column, line, num_of_columns, num_of_lines)) = Self::block_char_to_rect(ch) {
            Self::push_block_element_region(
                point,
                column,
                line,
                num_of_columns,
                num_of_lines,
                color,
                regions,
            );
            return true;
        }

        if let Some(filled) = Self::quadrant_char_to_filled_bits(ch) {
            for row in 0..2 {
                for column in 0..2 {
                    if filled & (1 << (row * 2 + column)) != 0 {
                        Self::push_block_element_region(
                            point,
                            column * 4,
                            row * 12,
                            4,
                            12,
                            color,
                            regions,
                        );
                    }
                }
            }
            return true;
        }

        if let Some(filled) = Self::sextant_char_to_filled_bits(ch) {
            for row in 0..3 {
                for column in 0..2 {
                    if filled & (1 << (row * 2 + column)) != 0 {
                        Self::push_block_element_region(
                            point,
                            column * 4,
                            row * 8,
                            4,
                            8,
                            color,
                            regions,
                        );
                    }
                }
            }
            return true;
        }

        if let Some(opacity) = Self::shade_char_to_opacity(ch) {
            Self::push_block_element_region(point, 0, 0, 8, 24, color.opacity(opacity), regions);
            return true;
        }

        false
    }

    fn push_block_element_region(
        point: LayoutPoint,
        column: i32,
        line: i32,
        num_of_columns: i32,
        num_of_lines: i32,
        color: Hsla,
        regions: &mut Vec<BackgroundRegion>,
    ) {
        let start_line = point.line * BLOCK_SUBCELL_LINES + line;
        let start_col = point.column * BLOCK_SUBCELL_COLUMNS + column;
        let end_line = start_line + num_of_lines - 1;
        let end_col = start_col + num_of_columns - 1;

        // Extend the previous region when possible (e.g. runs of `█` in a QR
        // code) to keep the quadratic merge pass over a small input.
        if let Some(last_region) = regions.last_mut()
            && last_region.color == color
            && last_region.start_line == start_line
            && last_region.end_line == end_line
            && last_region.end_col + 1 == start_col
        {
            last_region.end_col = end_col;
            return;
        }

        regions.push(BackgroundRegion::with_extents(
            start_line, start_col, end_line, end_col, color,
        ));
    }

    fn block_element_regions_to_rects(
        regions: Vec<BackgroundRegion>,
    ) -> Vec<BlockElementLayoutRect> {
        merge_background_regions(regions)
            .into_iter()
            .map(|region| {
                BlockElementLayoutRect::new(
                    LayoutPoint::new(region.start_line, region.start_col),
                    (region.end_col - region.start_col + 1) as usize,
                    (region.end_line - region.start_line + 1) as usize,
                    region.color,
                )
            })
            .collect()
    }

    /// Converts the Alacritty cell styles to GPUI text styles.
    fn cell_style(
        point: Point,
        cell: &Cell,
        fg: Color,
        bg: Color,
        colors: &TerminalColors,
        text_style: &TextStyle,
        hyperlink: Option<(HighlightStyle, &Range)>,
        minimum_contrast: f32,
    ) -> TextRun {
        let skip_contrast = is_app_chosen_exact_color(fg);
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
        window: &mut Window,
    ) {
        let focus = self.focus.clone();
        let terminal = self.terminal.clone();

        self.interactivity.on_mouse_down(MouseButton::Left, {
            let terminal = terminal.clone();
            let focus = focus.clone();

            move |e, window, cx| {
                window.focus(&focus, cx);

                terminal.update(cx, |terminal, cx| {
                    terminal.mouse_down(e, cx);
                    cx.notify();
                })
            }
        });

        window.on_mouse_event({
            let terminal = self.terminal.clone();
            let hitbox = hitbox.clone();
            let focus = focus.clone();
            move |e: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }

                if e.pressed_button.is_some() && !cx.has_active_drag() && focus.is_focused(window) {
                    let hovered = hitbox.is_hovered(window);

                    terminal.update(cx, |terminal, cx| {
                        if terminal.selection_started() || hovered {
                            terminal.mouse_drag(e, hitbox.bounds, cx);
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

        self.interactivity.on_scroll_wheel({
            let terminal_view = self.terminal_view.clone();
            move |e, _window, cx| {
                terminal_view.update(cx, |terminal_view, cx| {
                    terminal_view.scroll_wheel(e, cx);
                    cx.notify();
                });
            }
        });

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
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        // 独立终端：高度撑满父容器
        let height: Length = relative(1.).into();

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
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let settings = self.settings.clone();
        self.interactivity.prepaint(
            global_id,
            inspector_id,
            bounds,
            bounds.size,
            window,
            cx,
            |_, _, hitbox, window, cx| {
                let hitbox = hitbox.unwrap();
                let colors = settings.colors.clone();
                let font_size = settings.font_size;

                let text_style = TextStyle {
                    font_family: settings.font_family.clone(),
                    font_features: FontFeatures::disable_ligatures(),
                    font_weight: settings.font_weight,
                    font_fallbacks: None,
                    font_size: font_size.into(),
                    font_style: FontStyle::Normal,
                    line_height: px(
                        f32::from(font_size) * settings.line_height_multiplier,
                    )
                    .into(),
                    background_color: Some(colors.terminal_background),
                    white_space: WhiteSpace::Normal,
                    // These are going to be overridden per-cell
                    color: colors.terminal_foreground,
                    ..Default::default()
                };

                let text_system = window.text_system();
                let font_id = text_system.resolve_font(&text_style.font());
                let line_height_multiplier = settings.line_height_multiplier;
                let cell_width = text_system
                    .advance(font_id, font_size, 'm')
                    .unwrap()
                    .width;
                let line_height_px = f32::from(font_size) * line_height_multiplier;

                let mut grid_size = bounds.size;
                // https://github.com/zed-industries/zed/issues/2750
                // 1 列宽时渲染 🦀 会让 alacritty 出错（上游同样在视图层钳制）
                if grid_size.width < cell_width * 2.0 {
                    grid_size.width = cell_width * 2.0;
                }
                let available_height = grid_size.height;
                let mut origin = bounds.origin;

                // 独立终端：行高与可用高度按设备像素对齐；「底部锚定」时把不足一行的
                // padding 加在网格**顶部**，否则顶端固定（同 zed 上游 prepaint，那边
                // 整段包在 `TerminalMode::Standalone` 里）。锚定条件见下：ALT_SCREEN，
                // 或停在最新内容且视口最后一行被占用 —— 后者不能省，否则短内容时余量
                // 会随窗口高度上下移动。详见 `docs/terminal-view-rendering.md` §3.2.1。
                {
                    let should_anchor_to_bottom = {
                        let content = self.terminal.read(cx).last_content();
                        content.mode.contains(Modes::ALT_SCREEN)
                            || (content.scrolled_to_bottom && content.bottom_row_occupied)
                    };
                    let scale_factor = window.scale_factor();
                    let line_height_px = px(line_height_px);
                    let line_height_device_px =
                        (f32::from(line_height_px) * scale_factor).round().max(1.0) as i32;
                    let available_height_device_px =
                        (f32::from(available_height) * scale_factor).floor().max(0.0) as i32;

                    let rows =
                        ((available_height_device_px / line_height_device_px) as usize).max(1);
                    let snapped_height_device_px = (rows as i32) * line_height_device_px;
                    let padding_device_px =
                        (available_height_device_px - snapped_height_device_px).max(0);

                    let snapped_height =
                        px(snapped_height_device_px as f32 / scale_factor.max(1.0));
                    let padding = px(padding_device_px as f32 / scale_factor.max(1.0));

                    grid_size.height = snapped_height;
                    if should_anchor_to_bottom {
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

                let dimensions =
                    TerminalBounds::new(px(line_height_px), cell_width, Bounds { origin, size: grid_size });

                let search_matches = self.terminal.read(cx).matches.clone();

                let background_color = colors.terminal_background;

                self.terminal.update(cx, |terminal, cx| {
                    terminal.set_size(dimensions);
                    terminal.sync(window, cx);
                });

                let link_style = HighlightStyle {
                    color: Some(settings.link_color),
                    font_weight: Some(settings.font_weight),
                    font_style: None,
                    background_color: None,
                    underline: Some(UnderlineStyle {
                        thickness: px(1.0),
                        color: Some(settings.link_color),
                        wavy: false,
                    }),
                    strikethrough: None,
                    fade_out: None,
                };

                let content = self.terminal.read(cx).last_content();
                let mode = content.mode;
                let display_offset = content.display_offset;
                let cursor = content.cursor;
                let cursor_char = content.cursor_char;
                let selection = content.selection;
                let last_hovered_word = content.last_hovered_word.clone();

                // searches, highlights to a single range representations
                let mut relative_highlighted_ranges = Vec::new();
                for search_match in &search_matches {
                    relative_highlighted_ranges.push((*search_match, settings.search_match_color));
                }
                if let Some(selection) = selection {
                    relative_highlighted_ranges
                        .push((selection.point_range(), settings.selection_color));
                }

                // Calculate the intersection of the terminal's bounds with the current
                // content mask (the visible viewport after all parent clipping).
                // This allows us to only render cells that are actually visible.
                let content_bounds = dimensions.bounds;
                let visible_bounds = window.content_mask().bounds;
                let intersection = visible_bounds.intersect(&content_bounds);

                // If the terminal is entirely outside the viewport, skip all cell processing.
                let (rects, batched_text_runs, block_element_rects) = if intersection.size.height
                    <= px(0.)
                    || intersection.size.width <= px(0.)
                {
                    (Vec::new(), Vec::new(), Vec::new())
                } else if intersection == content_bounds {
                    // Fast path: terminal fully visible, no clipping needed.
                    TerminalElement::layout_grid(
                        content.cells.iter(),
                        0,
                        &text_style,
                        last_hovered_word
                            .as_ref()
                            .map(|last_hovered_word| (link_style, &last_hovered_word.word_match)),
                        settings.minimum_contrast,
                        &colors,
                        cx,
                    )
                } else {
                    // Calculate which screen rows are visible based on pixel positions.
                    // This works because we filter by screen position (enumerated line
                    // group index), not by the cell's internal line number.
                    let rows_above_viewport = f32::from(
                        (intersection.top() - content_bounds.top()).max(px(0.)) / line_height_px,
                    ) as usize;
                    let visible_row_count =
                        f32::from((intersection.size.height / line_height_px).ceil()) as usize + 1;

                    TerminalElement::layout_grid(
                        content
                            .cells
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
                        settings.minimum_contrast,
                        &colors,
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
                        font_size,
                        &[TextRun {
                            len,
                            font: text_style.font(),
                            color: colors.terminal_background,
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
                        let (kind, text) = match cursor.shape {
                            CursorShape::Block if !focused => (CursorKind::Hollow, None),
                            CursorShape::Block => (CursorKind::Block, Some(cursor_text)),
                            CursorShape::Underline if !focused => (CursorKind::Hollow, None),
                            CursorShape::Underline => (CursorKind::Underline, None),
                            CursorShape::Bar if !focused => (CursorKind::Hollow, None),
                            CursorShape::Bar => (CursorKind::Bar, None),
                            CursorShape::HollowBlock => (CursorKind::Hollow, None),
                            CursorShape::Hidden => unreachable!(),
                        };

                        CursorLayout {
                            origin: bounds.origin,
                            width: bounds.size.width,
                            height: bounds.size.height,
                            color: colors.terminal_foreground,
                            kind,
                            text,
                        }
                    })
                };

                LayoutState {
                    hitbox,
                    batched_text_runs,
                    block_element_rects,
                    cursor,
                    ime_cursor_bounds,
                    background_color,
                    dimensions,
                    rects,
                    relative_highlighted_ranges,
                    mode,
                    display_offset,
                    base_text_style: text_style,
                }
            },
        )
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        layout: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            window.paint_quad(fill(bounds, layout.background_color));
            let origin = layout.dimensions.bounds.origin;
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
                terminal_view: self.terminal_view.clone(),
                cursor_bounds: layout.ime_cursor_bounds.map(|bounds| bounds + origin),
            };

            self.register_mouse_listeners(layout.mode, &layout.hitbox, window);

            // 悬停在超链接上时显示手型光标
            let hovered_link = window.modifiers().secondary()
                && bounds.contains(&window.mouse_position())
                && self
                    .terminal
                    .read(cx)
                    .last_content()
                    .last_hovered_word
                    .is_some();
            if hovered_link {
                window.set_cursor_style(gpui::CursorStyle::PointingHand, &layout.hitbox);
            } else {
                window.set_cursor_style(gpui::CursorStyle::IBeam, &layout.hitbox);
            }

            let original_cursor = layout.cursor.take();
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

                    // 1. 背景矩形（cell 非默认背景）
                    for rect in &layout.rects {
                        rect.paint(origin, &layout.dimensions, window);
                    }

                    // 2. 选择 / 搜索高亮
                    paint_highlighted_ranges(
                        &layout.relative_highlighted_ranges,
                        layout,
                        origin,
                        window,
                    );

                    // 3. 批量文本 run
                    for batch in &layout.batched_text_runs {
                        batch.paint(origin, &layout.dimensions, window, cx);
                    }

                    // 4. 块字符矩形（subcell 网格）
                    for block_element_rect in &layout.block_element_rects {
                        block_element_rect.paint(origin, &layout.dimensions, window);
                    }

                    // 5. IME 组合文本
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

                        if let Err(error) = shaped_line.paint(
                            ime_position,
                            layout.dimensions.line_height,
                            gpui::TextAlign::Left,
                            None,
                            window,
                            cx,
                        ) {
                            log::error!("failed to paint ime text: {error}");
                        }
                    }

                    // 6. 光标
                    if self.cursor_visible
                        && marked_text_cloned.is_none()
                        && let Some(cursor) = original_cursor
                    {
                        cursor.paint(origin, window, cx);
                    }
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

/// 把高亮范围转换为逐行像素段并绘制（替代 Zed 的 `editor::HighlightedRange`）。
fn paint_highlighted_ranges(
    ranges: &[(Range, Hsla)],
    layout: &LayoutState,
    origin: GpuiPoint<Pixels>,
    window: &mut Window,
) {
    for (range, color) in ranges {
        let Some((start_y, lines)) = to_highlighted_range_lines(range, layout, origin) else {
            continue;
        };
        for (index, line) in lines.iter().enumerate() {
            let width = line.end_x - line.start_x;
            if width <= px(0.) {
                continue;
            }
            let y = start_y + index as f32 * layout.dimensions.line_height;
            window.paint_quad(fill(
                Bounds::new(
                    point(line.start_x, y),
                    size(width, layout.dimensions.line_height),
                ),
                *color,
            ));
        }
    }
}

/// 把 (start, end) 范围转换为视口内的逐行像素段。
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
pub fn convert_color(fg: &Color, colors: &TerminalColors) -> Hsla {
    match fg {
        // Named and theme defined colors
        Color::Named(named) => named_color(*named, colors),
        // 'True' colors
        Color::Spec(rgb) => rgba_color(rgb.r, rgb.g, rgb.b),
        // 8 bit, indexed colors
        Color::Indexed(i) => get_color_at_index(*i as usize, colors),
    }
}

fn named_color(named: NamedColor, colors: &TerminalColors) -> Hsla {
    use NamedColor::*;
    match named {
        Black => colors.terminal_ansi_black,
        Red => colors.terminal_ansi_red,
        Green => colors.terminal_ansi_green,
        Yellow => colors.terminal_ansi_yellow,
        Blue => colors.terminal_ansi_blue,
        Magenta => colors.terminal_ansi_magenta,
        Cyan => colors.terminal_ansi_cyan,
        White => colors.terminal_ansi_white,
        BrightBlack => colors.terminal_ansi_bright_black,
        BrightRed => colors.terminal_ansi_bright_red,
        BrightGreen => colors.terminal_ansi_bright_green,
        BrightYellow => colors.terminal_ansi_bright_yellow,
        BrightBlue => colors.terminal_ansi_bright_blue,
        BrightMagenta => colors.terminal_ansi_bright_magenta,
        BrightCyan => colors.terminal_ansi_bright_cyan,
        BrightWhite => colors.terminal_ansi_bright_white,
        DimBlack => colors.terminal_ansi_dim_black,
        DimRed => colors.terminal_ansi_dim_red,
        DimGreen => colors.terminal_ansi_dim_green,
        DimYellow => colors.terminal_ansi_dim_yellow,
        DimBlue => colors.terminal_ansi_dim_blue,
        DimMagenta => colors.terminal_ansi_dim_magenta,
        DimCyan => colors.terminal_ansi_dim_cyan,
        DimWhite => colors.terminal_ansi_dim_white,
        Foreground => colors.terminal_foreground,
        Background => colors.terminal_background,
        Cursor => colors.terminal_foreground,
        BrightForeground => colors.terminal_bright_foreground,
        DimForeground => colors.terminal_foreground,
    }
}

/// 判断 cell 是否为“空白”（空格 + 默认背景 + 无样式）。
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

struct TerminalInputHandler {
    terminal_view: Entity<TerminalView>,
    cursor_bounds: Option<Bounds<Pixels>>,
}

impl InputHandler for TerminalInputHandler {
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        _cx: &mut App,
    ) -> Option<UTF16Selection> {
        // Always return a valid selection for IME positioning,
        // even in ALT_SCREEN mode (fullscreen TUI apps like vim, etc.)
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(&mut self, _window: &mut Window, cx: &mut App) -> Option<StdRange<usize>> {
        self.terminal_view.read(cx).marked_text_range()
    }

    fn text_for_range(
        &mut self,
        _: StdRange<usize>,
        _: &mut Option<StdRange<usize>>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<String> {
        None
    }

    fn replace_text_in_range(
        &mut self,
        _replacement_range: Option<StdRange<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut App,
    ) {
        self.terminal_view.update(cx, |view, view_cx| {
            view.clear_marked_text(view_cx);
            view.commit_text(text, view_cx);
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<StdRange<usize>>,
        new_text: &str,
        _new_marked_range: Option<StdRange<usize>>,
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
        range_utf16: StdRange<usize>,
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
