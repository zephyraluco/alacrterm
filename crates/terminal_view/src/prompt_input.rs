//! Recover a literal, single-line input from the terminal after native readline edits.
//! Anchors are created only after a shell lifecycle Input marker, never by matching '$'.
use terminal::{Content, Point};
#[derive(Clone)]
pub(super) struct PromptAnchor {
    point: Point,
    prefix: String,
    columns: usize,
}
impl PromptAnchor {
    pub fn capture(content: &Content) -> Option<Self> {
        if content.cursor.point.line < 0 || content.display_offset != 0 {
            return None;
        }
        let point = content.cursor.point;
        let prefix = row(content, point.line, 0, point.column)?;
        Some(Self {
            point,
            prefix,
            columns: content.terminal_bounds.num_columns(),
        })
    }
    pub fn recover(&self, content: &Content) -> Option<(String, usize)> {
        let cursor = content.cursor.point;
        if content.display_offset != 0
            || cursor.line != self.point.line
            || cursor.column < self.point.column
            || content.terminal_bounds.num_columns() != self.columns
            || row(content, cursor.line, 0, self.point.column)? != self.prefix
        {
            return None;
        }
        let raw = row(content, cursor.line, self.point.column, self.columns)?;
        // Combining and wide characters need a shell-provided logical cursor model.
        if !raw.is_ascii() || raw.chars().any(char::is_control) {
            return None;
        }
        let at = cursor.column - self.point.column;
        let length = raw.trim_end_matches(' ').len().max(at);
        Some((raw.get(..length)?.into(), at))
    }
}
fn row(content: &Content, line: i32, start: usize, end: usize) -> Option<String> {
    let mut result = String::new();
    for column in start..end {
        let cell = content
            .cells
            .iter()
            .find(|cell| cell.point.line == line && cell.point.column == column)?;
        if cell.is_wide_char_spacer() {
            continue;
        }
        result.push(cell.character());
        if let Some(chars) = cell.zerowidth() {
            result.extend(chars);
        }
    }
    Some(result)
}
