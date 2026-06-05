use alacritty_terminal::{
    index::Point as AlacPoint, selection::SelectionRange, term::cell::Flags, vte::ansi::CursorShape,
};
use gpui::{Div, FontWeight, Hsla, div, prelude::*, px};
use terminal::{IndexedCell, TerminalContent, mappings::colors};

pub struct TerminalElement {
    content: TerminalContent,
    focused: bool,
    cursor_visible: bool,
}

impl TerminalElement {
    pub fn new(content: TerminalContent, focused: bool, cursor_visible: bool) -> Self {
        Self {
            content,
            focused,
            cursor_visible,
        }
    }

    pub fn render_div(self) -> Div {
        let bounds = self.content.terminal_bounds;
        let cursor_point = self.content.cursor.map(|c| c.point);
        // CursorShape::Hidden 表示终端自身隐藏光标，叠加闪烁与焦点状态
        let cursor_shape = self
            .content
            .cursor
            .map(|c| c.shape)
            .unwrap_or(CursorShape::Block);
        let cursor_visible =
            self.cursor_visible && self.focused && cursor_shape != CursorShape::Hidden;
        let selection = self.content.selection;
        let font_size = bounds.cell_width / 0.6;
        let visible_lines = bounds.num_lines() as i32;
        let default_bg = colors::DEFAULT_BG();
        // 滚动时 alacritty 返回负数行号（历史行）：display_row = line.0 + display_offset
        let display_offset = self.content.display_offset as i32;

        let rows = group_cells_by_line(self.content.cells);

        let children: Vec<Div> = rows
            .into_iter()
            .filter(|row| {
                row.first()
                    .map(|c| {
                        let dr = c.point.line.0 + display_offset;
                        dr >= 0 && dr < visible_lines
                    })
                    .unwrap_or(false)
            })
            .map(|row| {
                render_row(
                    row,
                    cursor_point,
                    cursor_visible,
                    cursor_shape,
                    bounds.line_height,
                    bounds.cell_width,
                    selection.clone(),
                    default_bg,
                )
            })
            .collect();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(default_bg)
            .font_family("JetBrainsMono Nerd Font")
            .text_size(px(font_size))
            .children(children)
    }
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

/// 双层分离渲染一行：
///
/// - **背景层**（flex-row，按 bg 合并）：无文字，flex 布局保证无间隙。
/// - **文字层**（absolute per-run，按 fg/bold/italic 合并，不含 bg）：
///   - 选区只改 bg，文字层不变 → 无跳动
///   - 跳过空格/null，绝对定位从 start_col 开始 → 无空白折叠问题
///   - 无宽度约束 → 无裁剪
///   - 相邻性检测：不跨空格合并，保证字符位置精确
fn render_row(
    cells: Vec<IndexedCell>,
    cursor_point: Option<AlacPoint>,
    cursor_visible: bool,
    cursor_shape: CursorShape,
    line_height: f32,
    cell_width: f32,
    selection: Option<SelectionRange>,
    default_bg: Hsla,
) -> Div {
    // ── 背景层 ────────────────────────────────────────────────────────────────
    struct BgSpan {
        col_count: usize,
        bg: Hsla,
    }
    let mut bg_spans: Vec<BgSpan> = Vec::new();

    for cell in &cells {
        let is_cursor = cursor_visible
            && cursor_shape == CursorShape::Block
            && cursor_point.map_or(false, |p| p == cell.point);
        let is_sel = selection
            .as_ref()
            .map_or(false, |s| is_point_selected(&cell.point, s));
        let (_, bg) = resolve_cell_colors(cell, is_cursor, is_sel);
        let cw: usize = if cell.flags.contains(Flags::WIDE_CHAR) {
            2
        } else {
            1
        };

        if let Some(last) = bg_spans.last_mut() {
            if last.bg == bg {
                last.col_count += cw;
                continue;
            }
        }
        bg_spans.push(BgSpan { col_count: cw, bg });
    }

    let bg_layer = div()
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .h_full()
        .flex()
        .flex_row()
        .children(bg_spans.into_iter().map(move |s| {
            div()
                .w(px(s.col_count as f32 * cell_width))
                .h_full()
                .flex_shrink_0()
                .bg(s.bg)
        }));

    // ── 文字层 ────────────────────────────────────────────────────────────────
    // 合并条件：fg/bold/italic 相同 + 列号连续（不跨空格）
    // 空格/null 跳过，bg 层负责颜色；不更新 last_text_end_col
    struct TextRun {
        text: String,
        start_col: usize,
        fg: Hsla,
        bold: bool,
        italic: bool,
    }
    let mut text_runs: Vec<TextRun> = Vec::new();
    // 上一个加入文字层的字符的列结束位置（不含跳过的空格）
    let mut last_text_end_col: usize = usize::MAX;

    for cell in &cells {
        let col = cell.point.column.0;
        let cw: usize = if cell.flags.contains(Flags::WIDE_CHAR) {
            2
        } else {
            1
        };
        let ch = cell.c;

        // 空格、null、FEFF 跳过——bg 层已负责颜色，无需文字节点
        // 注意：不更新 last_text_end_col，确保下一个可见字符不会错误合并
        if cell.flags.contains(Flags::HIDDEN) || ch == '\0' || ch == '\u{FEFF}' || ch == ' ' {
            continue;
        }

        let is_cursor = cursor_visible
            && cursor_shape == CursorShape::Block
            && cursor_point.map_or(false, |p| p == cell.point);
        let is_sel = selection
            .as_ref()
            .map_or(false, |s| is_point_selected(&cell.point, s));
        let (fg, _) = resolve_cell_colors(cell, is_cursor, is_sel);
        let bold = cell.flags.contains(Flags::BOLD);
        let italic = cell.flags.contains(Flags::ITALIC);

        // 相邻性：当前列 == 上一个文字字符的列结束位置
        let adjacent = col == last_text_end_col;

        if adjacent {
            if let Some(last) = text_runs.last_mut() {
                if last.fg == fg && last.bold == bold && last.italic == italic {
                    last.text.push(ch);
                    last_text_end_col = col + cw;
                    continue;
                }
            }
        }

        text_runs.push(TextRun {
            text: ch.to_string(),
            start_col: col,
            fg,
            bold,
            italic,
        });
        last_text_end_col = col + cw;
    }

    let cursor_layer = render_cursor_layer(
        cursor_point,
        cursor_visible,
        cursor_shape,
        cells.first().map(|cell| cell.point.line.0),
        line_height,
        cell_width,
    );

    div()
        .relative()
        .h(px(line_height))
        .flex_shrink_0()
        .overflow_hidden()
        .bg(default_bg)
        .child(bg_layer)
        .children(cursor_layer)
        .children(text_runs.into_iter().map(move |run| {
            let x = run.start_col as f32 * cell_width;
            div()
                .absolute()
                .top(px(0.0))
                .left(px(x))
                .h_full()
                .text_color(run.fg)
                .when(run.bold, |d| d.font_weight(FontWeight::BOLD))
                .when(run.italic, |d| d.italic())
                .child(run.text)
        }))
}

fn render_cursor_layer(
    cursor_point: Option<AlacPoint>,
    cursor_visible: bool,
    cursor_shape: CursorShape,
    row_line: Option<i32>,
    line_height: f32,
    cell_width: f32,
) -> Option<Div> {
    if !cursor_visible || cursor_shape == CursorShape::Block || cursor_shape == CursorShape::Hidden
    {
        return None;
    }

    let cursor_point = cursor_point?;
    if row_line != Some(cursor_point.line.0) {
        return None;
    }

    let x = cursor_point.column.0 as f32 * cell_width;
    let color = colors::CURSOR_COLOR();

    let cursor = match cursor_shape {
        CursorShape::Underline => div()
            .absolute()
            .left(px(x))
            .bottom(px(1.0))
            .w(px(cell_width))
            .h(px(2.0))
            .bg(color),
        CursorShape::Beam => div()
            .absolute()
            .left(px(x))
            .top(px(0.0))
            .w(px(2.0))
            .h(px(line_height))
            .bg(color),
        CursorShape::HollowBlock => div()
            .absolute()
            .left(px(x))
            .top(px(0.0))
            .w(px(cell_width))
            .h(px(line_height))
            .child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .w_full()
                    .h(px(1.0))
                    .bg(color),
            )
            .child(
                div()
                    .absolute()
                    .bottom(px(0.0))
                    .left(px(0.0))
                    .w_full()
                    .h(px(1.0))
                    .bg(color),
            )
            .child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .w(px(1.0))
                    .h_full()
                    .bg(color),
            )
            .child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .right(px(0.0))
                    .w(px(1.0))
                    .h_full()
                    .bg(color),
            ),
        CursorShape::Block | CursorShape::Hidden => return None,
    };

    Some(cursor)
}

fn resolve_cell_colors(cell: &IndexedCell, is_cursor: bool, is_selected: bool) -> (Hsla, Hsla) {
    let default_bg = colors::DEFAULT_BG();
    let cursor_color = colors::CURSOR_COLOR();
    let selection_bg = colors::SELECTION_BG();
    let is_bold = cell.flags.contains(Flags::BOLD);

    if cell.flags.contains(Flags::HIDDEN) {
        return (default_bg, default_bg);
    }

    let (fg, mut bg) = if cell.flags.contains(Flags::INVERSE) {
        (
            colors::resolve_color(&cell.bg, false),
            colors::resolve_color(&cell.fg, is_bold),
        )
    } else {
        (
            colors::resolve_color(&cell.fg, is_bold),
            colors::resolve_color(&cell.bg, false),
        )
    };

    if is_cursor {
        // 只改 bg，不反转 fg：
        // 1. fg 不变 → text run 不因光标位置而分裂 → 无亚像素跳动
        // 2. 光标仍清晰可见（光标色背景 + 自然前景色）
        bg = cursor_color;
    } else if is_selected {
        bg = selection_bg;
    }

    (fg, bg)
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
