//! Recover a literal, single-line input from the terminal after native readline edits.
//! Anchors are created only after a shell lifecycle Input marker, never by matching '$'.
use std::time::{Duration, Instant};
use terminal::{Content, Point};
pub(super) const RECOVERY_DELAY: Duration = Duration::from_millis(80);
#[derive(Clone)]
struct PromptAnchor {
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
/// Recovering always owns its prompt anchor, so it cannot outlive that prompt.
#[derive(Default)]
enum TrackingState {
    #[default]
    Unanchored,
    Anchored(PromptAnchor),
    Recovering {
        anchor: PromptAnchor,
        generation: u64,
        since: Instant,
    },
}
#[derive(Default)]
pub(super) struct InputTracking {
    state: TrackingState,
}
impl InputTracking {
    pub fn reset(&mut self) {
        self.state = TrackingState::Unanchored;
    }
    pub fn has_anchor(&self) -> bool {
        !matches!(self.state, TrackingState::Unanchored)
    }
    pub fn capture(&mut self, content: &Content) {
        if !self.has_anchor()
            && let Some(anchor) = PromptAnchor::capture(content)
        {
            self.state = TrackingState::Anchored(anchor);
        }
    }
    pub fn request(&mut self, generation: u64, since: Instant) -> bool {
        self.state = match std::mem::take(&mut self.state) {
            TrackingState::Anchored(anchor) | TrackingState::Recovering { anchor, .. } => {
                TrackingState::Recovering {
                    anchor,
                    generation,
                    since,
                }
            }
            TrackingState::Unanchored => return false,
        };
        true
    }
    pub fn due(&self, now: Instant) -> bool {
        matches!(&self.state, TrackingState::Recovering { since, .. } if now.saturating_duration_since(*since) >= RECOVERY_DELAY)
    }
    pub fn current(&self, content: &Content) -> Option<(String, usize)> {
        match &self.state {
            TrackingState::Anchored(anchor) | TrackingState::Recovering { anchor, .. } => {
                anchor.recover(content)
            }
            TrackingState::Unanchored => None,
        }
    }
    pub fn recover(
        &mut self,
        content: &Content,
        generation: u64,
        now: Instant,
    ) -> Option<(String, usize)> {
        if !self.due(now) {
            return None;
        }
        let TrackingState::Recovering {
            generation: previous,
            anchor,
            ..
        } = &self.state
        else {
            return None;
        };
        if *previous == generation {
            return None;
        }
        let input = anchor.recover(content)?;
        if let TrackingState::Recovering { anchor, .. } = std::mem::take(&mut self.state) {
            self.state = TrackingState::Anchored(anchor);
        }
        Some(input)
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
