//! Adapt line-based terminal history to the application's standard scrollbar.
use gpui::{Bounds, Pixels, Point, Size, point, px, size};
use gpui_kit::component::scroll::ScrollbarHandle;
use std::{cell::Cell, rc::Rc};

#[derive(Clone)]
pub(super) struct HistoryScrollbar {
    history: usize,
    offset: usize,
    line_height: Pixels,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    target: Rc<Cell<Option<usize>>>,
}
impl HistoryScrollbar {
    pub(super) fn new(
        history: usize,
        offset: usize,
        line_height: Pixels,
        target: Rc<Cell<Option<usize>>>,
    ) -> Self {
        Self {
            history,
            offset,
            line_height,
            bounds: Rc::default(),
            target,
        }
    }
    pub(super) fn set_bounds(&self, bounds: Bounds<Pixels>) {
        self.bounds.set(bounds);
    }
}

fn line_from_offset(offset: Pixels, line_height: Pixels, history: usize) -> usize {
    ((-offset / line_height.max(px(1.))).round().max(0.) as usize).min(history)
}

impl ScrollbarHandle for HistoryScrollbar {
    fn viewport_bounds(&self) -> Bounds<Pixels> {
        self.bounds.get()
    }
    fn content_size(&self) -> Size<Pixels> {
        let bounds = self.bounds.get();
        size(
            bounds.size.width,
            bounds.size.height + self.line_height * self.history as f32,
        )
    }
    fn offset(&self) -> Point<Pixels> {
        point(
            px(0.),
            -self.line_height * self.history.saturating_sub(self.offset) as f32,
        )
    }
    fn set_offset(&self, offset: Point<Pixels>) {
        // The widget notifies its owning TerminalView; its next render queues
        // this absolute position before the terminal's normal sync phase.
        self.target.set(Some(line_from_offset(
            offset.y,
            self.line_height,
            self.history,
        )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_positions_round_trip_and_dragging_clamps_to_valid_lines() {
        let target = Rc::default();
        let bar = HistoryScrollbar::new(100, 40, px(17.), target);
        bar.set_bounds(Bounds::new(point(px(0.), px(0.)), size(px(300.), px(200.))));
        assert_eq!(bar.content_size().height, px(1900.));
        assert_eq!(bar.offset().y, px(-1020.));
        bar.set_offset(bar.offset());
        assert_eq!(bar.target.take(), Some(60));
        bar.set_offset(point(px(0.), px(50.)));
        assert_eq!(bar.target.take(), Some(0));
        bar.set_offset(point(px(0.), px(-50000.)));
        assert_eq!(bar.target.take(), Some(100));
        assert_eq!(line_from_offset(px(-25.), px(17.), 100), 1);
        assert_eq!(line_from_offset(px(-26.), px(17.), 100), 2);
        assert_eq!(line_from_offset(px(-20.), px(0.), 100), 20);
    }
}
