use alacritty_terminal::{
    index::Point as AlacPoint,
    selection::SelectionRange,
    term::cell::Flags,
    vte::ansi::{Color as AnsiColor, CursorShape},
};
use gpui::{
    App, Bounds, ContentMask, Element, ElementId, ElementInputHandler, Entity, FocusHandle, Font,
    FontStyle, FontWeight, GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId,
    Pixels, ShapedLine, SharedString, StrikethroughStyle, Style, TextAlign, TextRun as GpuiTextRun,
    UnderlineStyle, Window, fill, point, px, relative, size,
};
use std::time::Instant;
use terminal::{
    IndexedCell, SearchMatch, Terminal, TerminalBounds, TerminalTheme, mappings::colors,
};

use crate::terminal_view::TerminalView;

pub struct TerminalElement {
    terminal: Entity<Terminal>,
    theme: TerminalTheme,
    font_family: SharedString,
    font_size: f32,
    line_height: f32,
    cursor_shape_override: Option<CursorShape>,
    minimum_contrast: f32,
    focused: bool,
    cursor_visible: bool,
    terminal_view: Entity<TerminalView>,
    focus_handle: FocusHandle,
    /// 基础字体粗细（400.0 = Normal，700.0 = Bold）
    font_weight: f32,
    /// IME 当前标记文字（用于输入法候选渲染）
    ime_marked_text: Option<String>,
}

pub struct LayoutState {
    bounds: Bounds<Pixels>,
    background_rects: Vec<LayoutRect>,
    text_runs: Vec<PaintTextRun>,
    cursor_rects: Vec<LayoutRect>,
    background: Hsla,
    line_height: f32,
    /// Block 光标下的字符（反色渲染，确保光标内字符可见）
    cursor_char_run: Option<PaintTextRun>,
    /// IME 标记文字的渲染信息
    ime_text_info: Option<ImeTextInfo>,
}

/// IME 标记文字渲染信息
struct ImeTextInfo {
    x: f32,
    y: f32,
    line: ShapedLine,
    background: Hsla,
}

const SEARCH_HIGHLIGHT: Hsla = Hsla {
    h: 0.12,
    s: 0.85,
    l: 0.45,
    a: 0.55,
};

#[derive(Clone, Copy)]
struct LayoutRect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    color: Hsla,
}

struct PaintTextRun {
    x: f32,
    y: f32,
    line: ShapedLine,
}

struct RawTextRun {
    text: String,
    start_col: usize,
    end_col: usize,
    display_row: i32,
    fg: Hsla,
    bold: bool,
    italic: bool,
    underline: bool,
    undercurl: bool,
    strikeout: bool,
}

impl TerminalElement {
    pub fn new(
        terminal: Entity<Terminal>,
        theme: TerminalTheme,
        font_family: impl Into<SharedString>,
        font_size: f32,
        line_height: f32,
        cursor_shape_override: Option<CursorShape>,
        minimum_contrast: f32,
        focused: bool,
        cursor_visible: bool,
        terminal_view: Entity<TerminalView>,
        focus_handle: FocusHandle,
        font_weight: f32,
        ime_marked_text: Option<String>,
    ) -> Self {
        Self {
            terminal,
            theme,
            font_family: font_family.into(),
            font_size,
            line_height,
            cursor_shape_override,
            minimum_contrast,
            focused,
            cursor_visible,
            terminal_view,
            focus_handle,
            font_weight,
            ime_marked_text,
        }
    }
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = LayoutState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        element_bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        let layout_start = Instant::now();

        let dimensions = terminal_dimensions(
            element_bounds,
            self.font_size,
            self.line_height,
            &self.font_family,
            window,
        );
        let content = self.terminal.update(_cx, |terminal, cx| {
            terminal.set_theme(self.theme);
            terminal.resize(dimensions);
            terminal.sync(cx);
            terminal.last_content.clone()
        });
        let bounds = content.terminal_bounds;
        let display_offset = content.display_offset as i32;
        self.terminal_view.update(_cx, |view, _cx| {
            view.update_terminal_layout(bounds, content.display_offset);
        });
        let cursor_point = content.cursor.map(|cursor| cursor.point);
        let cursor_shape = self
            .cursor_shape_override
            .or_else(|| content.cursor.map(|cursor| cursor.shape))
            .unwrap_or(CursorShape::Block);
        let cursor_visible = self.cursor_visible && cursor_shape != CursorShape::Hidden;
        let effective_cursor_shape = if cursor_visible && !self.focused {
            CursorShape::HollowBlock
        } else {
            cursor_shape
        };
        let cursor_width = cursor_width(
            content.cursor_char,
            self.font_size,
            bounds.cell_width,
            &self.font_family,
            window,
        );
        let visible_lines = bounds.num_lines() as i32;
        let rows = group_cells_by_line(content.cells);
        let visible_bounds = window.content_mask().bounds.intersect(&bounds.bounds);
        let first_visible_row = if visible_bounds.size.height <= px(0.) {
            visible_lines
        } else {
            ((f32::from((visible_bounds.top() - bounds.bounds.top()).max(px(0.)))
                / bounds.line_height)
                .floor() as i32)
                .max(0)
        };
        let last_visible_row = if visible_bounds.size.height <= px(0.) {
            -1
        } else {
            ((f32::from((visible_bounds.bottom() - bounds.bounds.top()).max(px(0.)))
                / bounds.line_height)
                .ceil() as i32)
                .min(visible_lines - 1)
        };

        let estimated_cells = rows.iter().map(Vec::len).sum::<usize>();
        let mut background_rects = Vec::with_capacity(estimated_cells / 8);
        let mut raw_text_runs = Vec::with_capacity(estimated_cells / 6);
        let mut cursor_rects = Vec::new();

        for row in rows {
            let Some(first_cell) = row.first() else {
                continue;
            };
            let display_row = first_cell.point.line.0 + display_offset;
            if display_row < first_visible_row || display_row > last_visible_row {
                continue;
            }

            let row_y = display_row as f32 * bounds.line_height;
            layout_row(
                row,
                display_row,
                row_y,
                cursor_point,
                cursor_visible,
                effective_cursor_shape,
                bounds.line_height,
                bounds.cell_width,
                content.selection,
                self.theme,
                self.minimum_contrast,
                &mut background_rects,
                &mut raw_text_runs,
            );
        }

        add_search_highlights(
            &content.search_matches,
            display_offset,
            first_visible_row,
            last_visible_row,
            bounds.line_height,
            bounds.cell_width,
            &mut background_rects,
        );

        layout_cursor(
            cursor_point,
            cursor_visible,
            effective_cursor_shape,
            display_offset,
            visible_lines,
            bounds.line_height,
            bounds.cell_width,
            cursor_width,
            self.theme,
            &mut cursor_rects,
        );

        let background_rects = merge_background_rects(background_rects);
        let text_runs = build_paint_text_runs(
            raw_text_runs,
            &self.font_family,
            self.font_size,
            self.font_weight,
            bounds.cell_width,
            bounds.line_height,
            window,
        );

        // ── Block 光标字符渲染：在光标位置用背景色绘制字符（保证可读性）─────────
        let cursor_char_run = if cursor_visible
            && (effective_cursor_shape == CursorShape::Block)
        {
            cursor_point.and_then(|cursor_pt| {
                let display_row = cursor_pt.line.0 + display_offset;
                if display_row < 0 || display_row >= visible_lines {
                    return None;
                }
                let ch = content.cursor_char;
                if ch.is_whitespace() || ch == '\0' {
                    return None;
                }
                let run_text = ch.to_string();
                let font = Font {
                    family: self.font_family.clone(),
                    ..Default::default()
                };
                // 反色：用终端背景色作为字符颜色，确保在光标色块上可见
                let style = GpuiTextRun {
                    len: run_text.len(),
                    font,
                    color: self.theme.background,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let line = window.text_system().shape_line(
                    run_text.into(),
                    px(self.font_size),
                    &[style],
                    Some(px(bounds.cell_width)),
                );
                Some(PaintTextRun {
                    x: cursor_pt.column.0 as f32 * bounds.cell_width,
                    y: display_row as f32 * bounds.line_height,
                    line,
                })
            })
        } else {
            None
        };

        // ── IME 标记文字渲染信息 ──────────────────────────────────────────────
        let ime_text_info = self.ime_marked_text.as_deref().and_then(|marked| {
            if marked.is_empty() {
                return None;
            }
            let cursor_pt = cursor_point?;
            let display_row = cursor_pt.line.0 + display_offset;
            if display_row < 0 || display_row >= visible_lines {
                return None;
            }
            let font = Font {
                family: self.font_family.clone(),
                ..Default::default()
            };
            let style = GpuiTextRun {
                len: marked.len(),
                font,
                color: self.theme.foreground,
                background_color: None,
                underline: Some(UnderlineStyle {
                    color: Some(self.theme.foreground),
                    thickness: px(1.0),
                    wavy: false,
                }),
                strikethrough: None,
            };
            let line = window.text_system().shape_line(
                marked.to_string().into(),
                px(self.font_size),
                &[style],
                None,
            );
            Some(ImeTextInfo {
                x: cursor_pt.column.0 as f32 * bounds.cell_width,
                y: display_row as f32 * bounds.line_height,
                line,
                background: self.theme.selection,
            })
        });

        log::debug!(
            "Terminal prepaint: layout took {:?}",
            layout_start.elapsed()
        );

        LayoutState {
            bounds: bounds.bounds,
            background_rects,
            text_runs,
            cursor_rects,
            background: self.theme.background,
            line_height: bounds.line_height,
            cursor_char_run,
            ime_text_info,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        layout: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let paint_start = Instant::now();
        let snapped_bounds = snap_bounds(layout.bounds, window.scale_factor());
        window.handle_input(
            &self.focus_handle,
            ElementInputHandler::new(snapped_bounds, self.terminal_view.clone()),
            cx,
        );
        window.with_content_mask(
            Some(ContentMask {
                bounds: snapped_bounds,
            }),
            |window| {
                window.paint_quad(fill(snapped_bounds, layout.background));

                for rect in &layout.background_rects {
                    paint_rect(rect, snapped_bounds, window);
                }

                for rect in &layout.cursor_rects {
                    paint_rect(rect, snapped_bounds, window);
                }

                for run in &layout.text_runs {
                    let origin = snapped_bounds.origin + point(px(run.x), px(run.y));
                    let _ = run.line.paint(
                        origin,
                        px(layout.line_height),
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }

                // Block 光标字符（反色，叠加在光标矩形之上）
                if let Some(run) = &layout.cursor_char_run {
                    let origin = snapped_bounds.origin + point(px(run.x), px(run.y));
                    let _ = run.line.paint(
                        origin,
                        px(layout.line_height),
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }

                // IME 标记文字（背景 + 文字）
                if let Some(ime) = &layout.ime_text_info {
                    let origin = snapped_bounds.origin + point(px(ime.x), px(ime.y));
                    let ime_width: f32 = ime.line.width().into();
                    let bg_bounds = Bounds {
                        origin,
                        size: size(px(ime_width), px(layout.line_height)),
                    };
                    window.paint_quad(fill(bg_bounds, ime.background));
                    let _ = ime.line.paint(
                        origin,
                        px(layout.line_height),
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }
            },
        );

        log::debug!(
            "Terminal paint: {} text runs, {} bg rects, took {:?}",
            layout.text_runs.len(),
            layout.background_rects.len(),
            paint_start.elapsed()
        );
    }
}

fn terminal_dimensions(
    element_bounds: Bounds<Pixels>,
    font_size: f32,
    line_height: f32,
    font_family: &SharedString,
    window: &mut Window,
) -> TerminalBounds {
    let font = Font {
        family: font_family.clone(),
        ..Default::default()
    };
    let font_id = window.text_system().resolve_font(&font);
    let cell_width = window
        .text_system()
        .advance(font_id, px(font_size), 'm')
        .map(|advance| f32::from(advance.width))
        .unwrap_or(font_size * 0.6)
        .max(1.0);
    // Gutter：左侧保留一个字符宽度，防止宽字符（emoji）导致 alacritty 渲染错乱
    let gutter = cell_width;
    let line_height = line_height.max(font_size);
    let scale_factor = window.scale_factor().max(1.0);
    let line_height_device_px = (line_height * scale_factor).round().max(1.0);
    let available_height_device_px = (f32::from(element_bounds.size.height) * scale_factor)
        .floor()
        .max(0.0);
    let rows = (available_height_device_px / line_height_device_px)
        .floor()
        .max(1.0);
    let snapped_height = rows * line_height_device_px / scale_factor;
    // 可用宽度减去 gutter
    let available_width = (f32::from(element_bounds.size.width) - gutter).max(0.0);
    let columns = (available_width / cell_width).floor().max(1.0);
    let snapped_width = columns * cell_width;
    let snap_px = |value: Pixels| px((f32::from(value) * scale_factor).floor() / scale_factor);

    TerminalBounds::new(
        cell_width,
        line_height,
        Bounds {
            origin: point(
                // 原点向右偏移 gutter，网格从 gutter 处开始
                snap_px(element_bounds.origin.x + px(gutter)),
                snap_px(element_bounds.origin.y),
            ),
            size: size(px(snapped_width), px(snapped_height)),
        },
    )
}

fn snap_bounds(mut bounds: Bounds<Pixels>, scale_factor: f32) -> Bounds<Pixels> {
    let snap_down = |value: Pixels| px((f32::from(value) * scale_factor).floor() / scale_factor);
    let snap_up = |value: Pixels| px((f32::from(value) * scale_factor).ceil() / scale_factor);
    let right = snap_up(bounds.right());
    let bottom = snap_up(bounds.bottom());
    bounds.origin.x = snap_down(bounds.origin.x);
    bounds.origin.y = snap_down(bounds.origin.y);
    bounds.size.width = (right - bounds.origin.x).max(px(0.));
    bounds.size.height = (bottom - bounds.origin.y).max(px(0.));
    bounds
}

fn paint_rect(rect: &LayoutRect, bounds: Bounds<Pixels>, window: &mut Window) {
    window.paint_quad(fill(
        Bounds {
            origin: bounds.origin + point(px(rect.x), px(rect.y)),
            size: size(px(rect.width), px(rect.height)),
        },
        rect.color,
    ));
}

fn group_cells_by_line(cells: Vec<IndexedCell>) -> Vec<Vec<IndexedCell>> {
    let mut rows: Vec<Vec<IndexedCell>> = Vec::new();
    let mut current: Vec<IndexedCell> = Vec::new();
    let mut cur_line: i32 = i32::MIN;

    for cell in cells {
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        let line = cell.point.line.0;
        if line != cur_line {
            if !current.is_empty() {
                rows.push(std::mem::take(&mut current));
            }
            cur_line = line;
        }
        current.push(cell);
    }
    if !current.is_empty() {
        rows.push(current);
    }
    rows
}

#[allow(clippy::too_many_arguments)]
fn layout_row(
    cells: Vec<IndexedCell>,
    display_row: i32,
    row_y: f32,
    cursor_point: Option<AlacPoint>,
    cursor_visible: bool,
    cursor_shape: CursorShape,
    line_height: f32,
    cell_width: f32,
    selection: Option<SelectionRange>,
    theme: TerminalTheme,
    minimum_contrast: f32,
    background_rects: &mut Vec<LayoutRect>,
    text_runs: &mut Vec<RawTextRun>,
) {
    struct BgSpan {
        start_col: usize,
        col_count: usize,
        bg: Hsla,
    }

    let mut bg_spans: Vec<BgSpan> = Vec::new();
    for cell in &cells {
        let col = cell.point.column.0;
        let is_cursor = cursor_visible
            && cursor_shape == CursorShape::Block
            && cursor_point.is_some_and(|point| point == cell.point);
        let is_sel = selection
            .as_ref()
            .is_some_and(|selection| is_point_selected(&cell.point, selection));
        let (_, bg) =
            resolve_cell_colors(cell, cell.c, is_cursor, is_sel, &theme, minimum_contrast);
        let cw = cell_width_columns(cell);

        if let Some(last) = bg_spans.last_mut() {
            if last.bg == bg && last.start_col + last.col_count == col {
                last.col_count += cw;
                continue;
            }
        }

        bg_spans.push(BgSpan {
            start_col: col,
            col_count: cw,
            bg,
        });
    }

    background_rects.extend(
        bg_spans
            .into_iter()
            .filter(|span| span.bg != theme.background)
            .map(|span| LayoutRect {
                x: span.start_col as f32 * cell_width,
                y: row_y,
                width: span.col_count as f32 * cell_width,
                height: line_height,
                color: span.bg,
            }),
    );

    let mut last_text_end_col = usize::MAX;
    for cell in &cells {
        let col = cell.point.column.0;
        let cw = cell_width_columns(cell);
        let ch = cell.c;

        if cell.flags.contains(Flags::HIDDEN) || ch == '\0' || ch == '\u{FEFF}' || ch == ' ' {
            continue;
        }

        let is_cursor = cursor_visible
            && cursor_shape == CursorShape::Block
            && cursor_point.is_some_and(|point| point == cell.point);
        let is_sel = selection
            .as_ref()
            .is_some_and(|selection| is_point_selected(&cell.point, selection));
        let (fg, _) = resolve_cell_colors(cell, ch, is_cursor, is_sel, &theme, minimum_contrast);
        let bold = cell.flags.contains(Flags::BOLD);
        let italic = cell.flags.contains(Flags::ITALIC);
        let underline = cell.flags.intersects(Flags::ALL_UNDERLINES);
        let undercurl = cell.flags.contains(Flags::UNDERCURL);
        let strikeout = cell.flags.intersects(Flags::STRIKEOUT);

        if col == last_text_end_col {
            if let Some(last) = text_runs.last_mut() {
                if last.display_row == display_row
                    && last.fg == fg
                    && last.bold == bold
                    && last.italic == italic
                    && last.underline == underline
                    && last.undercurl == undercurl
                    && last.strikeout == strikeout
                {
                    last.text.push(ch);
                    if let Some(chars) = cell.zerowidth() {
                        last.text.extend(chars.iter().copied());
                    }
                    last.end_col = col + cw;
                    last_text_end_col = col + cw;
                    continue;
                }
            }
        }

        let mut text = ch.to_string();
        if let Some(chars) = cell.zerowidth() {
            text.extend(chars.iter().copied());
        }

        text_runs.push(RawTextRun {
            text,
            start_col: col,
            end_col: col + cw,
            display_row,
            fg,
            bold,
            italic,
            underline,
            undercurl,
            strikeout,
        });
        last_text_end_col = col + cw;
    }
}

fn merge_background_rects(mut rects: Vec<LayoutRect>) -> Vec<LayoutRect> {
    if rects.is_empty() {
        return rects;
    }

    let mut changed = true;
    while changed {
        changed = false;
        'outer: for i in 0..rects.len() {
            for j in (i + 1)..rects.len() {
                if rects[i].color != rects[j].color {
                    continue;
                }

                let same_row = (rects[i].y - rects[j].y).abs() < 0.01
                    && (rects[i].height - rects[j].height).abs() < 0.01;
                let horizontally_adjacent = (rects[i].x + rects[i].width - rects[j].x).abs() < 0.01
                    || (rects[j].x + rects[j].width - rects[i].x).abs() < 0.01;
                if same_row && horizontally_adjacent {
                    let left = rects[i].x.min(rects[j].x);
                    let right = (rects[i].x + rects[i].width).max(rects[j].x + rects[j].width);
                    rects[i].x = left;
                    rects[i].width = right - left;
                    rects.remove(j);
                    changed = true;
                    break 'outer;
                }

                let same_col = (rects[i].x - rects[j].x).abs() < 0.01
                    && (rects[i].width - rects[j].width).abs() < 0.01;
                let vertically_adjacent = (rects[i].y + rects[i].height - rects[j].y).abs() < 0.01
                    || (rects[j].y + rects[j].height - rects[i].y).abs() < 0.01;
                if same_col && vertically_adjacent {
                    let top = rects[i].y.min(rects[j].y);
                    let bottom = (rects[i].y + rects[i].height).max(rects[j].y + rects[j].height);
                    rects[i].y = top;
                    rects[i].height = bottom - top;
                    rects.remove(j);
                    changed = true;
                    break 'outer;
                }
            }
        }
    }

    rects
}

fn add_search_highlights(
    matches: &[SearchMatch],
    display_offset: i32,
    first_visible_row: i32,
    last_visible_row: i32,
    line_height: f32,
    cell_width: f32,
    background_rects: &mut Vec<LayoutRect>,
) {
    for search_match in matches {
        let start_line = search_match.start.line.0;
        let end_line = search_match.end.line.0;
        for line in start_line..=end_line {
            let display_row = line + display_offset;
            if display_row < first_visible_row || display_row > last_visible_row {
                continue;
            }

            let start_col = if line == start_line {
                search_match.start.column.0
            } else {
                0
            };
            let end_col = if line == end_line {
                search_match.end.column.0.saturating_add(1)
            } else {
                start_col.saturating_add(1)
            };
            let col_count = end_col.saturating_sub(start_col).max(1);

            background_rects.push(LayoutRect {
                x: start_col as f32 * cell_width,
                y: display_row as f32 * line_height,
                width: col_count as f32 * cell_width,
                height: line_height,
                color: SEARCH_HIGHLIGHT,
            });
        }
    }
}

fn build_paint_text_runs(
    mut raw_runs: Vec<RawTextRun>,
    font_family: &SharedString,
    font_size: f32,
    font_weight: f32,
    cell_width: f32,
    line_height: f32,
    window: &mut Window,
) -> Vec<PaintTextRun> {
    if raw_runs.is_empty() {
        return Vec::new();
    }

    raw_runs.sort_by(|a, b| {
        a.display_row
            .cmp(&b.display_row)
            .then(a.start_col.cmp(&b.start_col))
    });

    let mut painted = Vec::with_capacity(raw_runs.len().min(line_height as usize + 1));
    let mut index = 0;
    while index < raw_runs.len() {
        let display_row = raw_runs[index].display_row;
        let start_col = raw_runs[index].start_col;
        let mut col = start_col;
        let mut text = String::new();
        let mut styles = Vec::new();

        while index < raw_runs.len() && raw_runs[index].display_row == display_row {
            let run = &raw_runs[index];
            if run.start_col > col {
                let gap = run.start_col - col;
                text.extend(std::iter::repeat_n(' ', gap));
                styles.push(gpui_gap_run(run, gap, font_family, font_weight));
            }

            text.push_str(&run.text);
            styles.push(gpui_text_run(run, run.text.len(), font_family, font_weight));
            col = run.end_col;
            index += 1;
        }

        let line = window.text_system().shape_line(
            text.into(),
            px(font_size),
            &styles,
            Some(px(cell_width)),
        );
        painted.push(PaintTextRun {
            x: start_col as f32 * cell_width,
            y: display_row as f32 * line_height,
            line,
        });
    }

    painted
}

fn cursor_width(
    cursor_char: char,
    font_size: f32,
    cell_width: f32,
    font_family: &SharedString,
    window: &mut Window,
) -> f32 {
    if cursor_char.is_whitespace() || cursor_char == '\0' {
        return cell_width;
    }

    let text = cursor_char.to_string();
    let font = Font {
        family: font_family.clone(),
        ..Default::default()
    };
    let style = GpuiTextRun {
        len: text.len(),
        font,
        color: gpui::white(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line =
        window
            .text_system()
            .shape_line(text.into(), px(font_size), &[style], Some(px(cell_width)));
    f32::from(line.width()).max(cell_width)
}

fn gpui_text_run(
    run: &RawTextRun,
    len: usize,
    font_family: &SharedString,
    base_font_weight: f32,
) -> GpuiTextRun {
    let font = Font {
        family: font_family.clone(),
        weight: if run.bold {
            FontWeight::BOLD
        } else {
            FontWeight(base_font_weight)
        },
        style: if run.italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        },
        ..Default::default()
    };

    GpuiTextRun {
        len,
        font,
        color: run.fg,
        background_color: None,
        underline: run.underline.then(|| UnderlineStyle {
            color: Some(run.fg),
            thickness: px(1.0),
            wavy: run.undercurl,
        }),
        strikethrough: run.strikeout.then(|| StrikethroughStyle {
            color: Some(run.fg),
            thickness: px(1.0),
        }),
    }
}

fn gpui_gap_run(
    run: &RawTextRun,
    len: usize,
    font_family: &SharedString,
    base_font_weight: f32,
) -> GpuiTextRun {
    let mut style = gpui_text_run(run, len, font_family, base_font_weight);
    style.underline = None;
    style.strikethrough = None;
    style
}

#[allow(clippy::too_many_arguments)]
fn layout_cursor(
    cursor_point: Option<AlacPoint>,
    cursor_visible: bool,
    cursor_shape: CursorShape,
    display_offset: i32,
    visible_lines: i32,
    line_height: f32,
    cell_width: f32,
    cursor_width: f32,
    theme: TerminalTheme,
    cursor_rects: &mut Vec<LayoutRect>,
) {
    if !cursor_visible || cursor_shape == CursorShape::Block || cursor_shape == CursorShape::Hidden
    {
        return;
    }

    let Some(cursor_point) = cursor_point else {
        return;
    };
    let display_row = cursor_point.line.0 + display_offset;
    if display_row < 0 || display_row >= visible_lines {
        return;
    }

    let x = cursor_point.column.0 as f32 * cell_width;
    let y = display_row as f32 * line_height;
    let color = theme.cursor;

    match cursor_shape {
        CursorShape::Underline => cursor_rects.push(LayoutRect {
            x,
            y: y + line_height - 2.0,
            width: cursor_width,
            height: 2.0,
            color,
        }),
        CursorShape::Beam => cursor_rects.push(LayoutRect {
            x,
            y,
            width: 2.0,
            height: line_height,
            color,
        }),
        CursorShape::HollowBlock => {
            cursor_rects.push(LayoutRect {
                x,
                y,
                width: cursor_width,
                height: 1.0,
                color,
            });
            cursor_rects.push(LayoutRect {
                x,
                y: y + line_height - 1.0,
                width: cursor_width,
                height: 1.0,
                color,
            });
            cursor_rects.push(LayoutRect {
                x,
                y,
                width: 1.0,
                height: line_height,
                color,
            });
            cursor_rects.push(LayoutRect {
                x: x + cursor_width - 1.0,
                y,
                width: 1.0,
                height: line_height,
                color,
            });
        }
        CursorShape::Block | CursorShape::Hidden => {}
    }
}

fn cell_width_columns(cell: &IndexedCell) -> usize {
    if cell.flags.contains(Flags::WIDE_CHAR) {
        2
    } else {
        1
    }
}

fn resolve_cell_colors(
    cell: &IndexedCell,
    ch: char,
    is_cursor: bool,
    is_selected: bool,
    theme: &TerminalTheme,
    minimum_contrast: f32,
) -> (Hsla, Hsla) {
    let default_bg = theme.background;
    let cursor_color = theme.cursor;
    let selection_bg = theme.selection;
    let is_bold = cell.flags.contains(Flags::BOLD);

    if cell.flags.contains(Flags::HIDDEN) {
        return (default_bg, default_bg);
    }

    let raw_fg = if cell.flags.contains(Flags::INVERSE) {
        &cell.bg
    } else {
        &cell.fg
    };

    let (mut fg, mut bg) = if cell.flags.contains(Flags::INVERSE) {
        (
            colors::resolve_color(&cell.bg, false, theme),
            colors::resolve_color(&cell.fg, is_bold, theme),
        )
    } else {
        (
            colors::resolve_color(&cell.fg, is_bold, theme),
            colors::resolve_color(&cell.bg, false, theme),
        )
    };

    if cell.flags.contains(Flags::DIM) {
        fg.a *= 0.7;
    }

    if !is_app_chosen_exact_color(raw_fg) && !is_decorative_character(ch) {
        fg = ensure_minimum_contrast(fg, bg, minimum_contrast);
    }

    if is_cursor {
        bg = cursor_color;
    } else if is_selected {
        bg = selection_bg;
    }

    (fg, bg)
}

fn is_app_chosen_exact_color(color: &AnsiColor) -> bool {
    matches!(color, AnsiColor::Spec(_) | AnsiColor::Indexed(16..=255))
}

fn is_decorative_character(ch: char) -> bool {
    matches!(
        ch as u32,
        0x2500..=0x257F
            | 0x2580..=0x259F
            | 0x25A0..=0x25FF
            | 0xE0B0..=0xE0B7
            | 0xE0B8..=0xE0BF
            | 0xE0C0..=0xE0CA
            | 0xE0CC..=0xE0D1
            | 0xE0D2..=0xE0D7
    )
}

/// APCA (Advanced Perceptual Contrast Algorithm) 对比度
/// 返回 Lc 值（绝对值范围约 0–108），比 WCAG 相对亮度比更接近人眼感知。
/// minimum_contrast 应使用 APCA Lc 值（默认 45.0 ≈ WCAG 4.5）。
fn apca_contrast(fg: Hsla, bg: Hsla) -> f32 {
    let srgb_to_linear = |c: f32| -> f32 {
        let c = c.clamp(0.0, 1.0);
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = |color: Hsla| -> f32 {
        let rgba: gpui::Rgba = color.into();
        let r = srgb_to_linear(rgba.r);
        let g = srgb_to_linear(rgba.g);
        let b = srgb_to_linear(rgba.b);
        // 最小钳位防止全黑产生数学奇点
        (0.2126 * r + 0.7152 * g + 0.0722 * b).max(0.0)
    };
    let y_bg = luminance(bg);
    let y_fg = luminance(fg);
    // APCA-W3 正极性（深色文字在浅色背景）vs 反极性（浅色文字在深色背景）
    if y_bg >= y_fg {
        (y_bg.powf(0.56) - y_fg.powf(0.57)) * 1.14 * 100.0
    } else {
        (y_bg.powf(0.65) - y_fg.powf(0.62)) * 1.14 * 100.0
    }
}

fn ensure_minimum_contrast(fg: Hsla, bg: Hsla, minimum: f32) -> Hsla {
    if apca_contrast(fg, bg).abs() >= minimum {
        return fg;
    }

    let fg_rgba: gpui::Rgba = fg.into();
    let bg_rgba: gpui::Rgba = bg.into();
    // 根据背景亮度选择调整目标（深背景 → 调亮前景，浅背景 → 调暗前景）
    let bg_approx_lum = 0.2126 * bg_rgba.r + 0.7152 * bg_rgba.g + 0.0722 * bg_rgba.b;
    let target = if bg_approx_lum < 0.5 {
        gpui::Rgba { r: 1.0, g: 1.0, b: 1.0, a: fg.a }
    } else {
        gpui::Rgba { r: 0.0, g: 0.0, b: 0.0, a: fg.a }
    };

    for step in 1..=10 {
        let amount = step as f32 / 10.0;
        let candidate: Hsla = mix_rgba(fg_rgba, target, amount).into();
        if apca_contrast(candidate, bg).abs() >= minimum {
            return candidate;
        }
    }

    target.into()
}

fn mix_rgba(a: gpui::Rgba, b: gpui::Rgba, amount: f32) -> gpui::Rgba {
    gpui::Rgba {
        r: a.r + (b.r - a.r) * amount,
        g: a.g + (b.g - a.g) * amount,
        b: a.b + (b.b - a.b) * amount,
        a: a.a,
    }
}

fn is_point_selected(point: &AlacPoint, selection: &SelectionRange) -> bool {
    let (start, end) = if selection.start <= selection.end {
        (selection.start, selection.end)
    } else {
        (selection.end, selection.start)
    };

    if selection.is_block {
        let min_col = start.column.min(end.column);
        let max_col = start.column.max(end.column);
        point.line >= start.line
            && point.line <= end.line
            && point.column >= min_col
            && point.column <= max_col
    } else {
        (*point >= start) && (*point <= end)
    }
}
