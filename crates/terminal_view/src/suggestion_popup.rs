//! Cursor-anchored history menu. Accepting a candidate only writes its suffix.
use crate::TerminalView;
use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement as _, MouseButton,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, div, px,
};
use terminal::Modes;
impl TerminalView {
    pub(crate) fn render_suggestions(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.focused
            || self.ime_state.is_some()
            || !self.suggestions.active
            || self.suggestions.candidates.is_empty()
        {
            return None;
        }
        let terminal = self.terminal.as_ref()?.read(cx);
        let content = terminal.last_content();
        if content.display_offset != 0
            || terminal.vi_mode_enabled()
            || content
                .mode
                .intersects(Modes::ALT_SCREEN | Modes::MOUSE_MODE)
        {
            return None;
        }
        let bounds = content.terminal_bounds;
        let width = bounds.bounds.size.width.min(px(420.)).max(px(0.));
        if width < px(120.) {
            return None;
        }
        let row_height = px(28.);
        let height = row_height * (self.suggestions.candidates.len() + 1) as f32 + px(8.);
        let caret_y = bounds.line_height * content.cursor.point.line.max(0) as f32;
        let below = caret_y + bounds.line_height;
        let top = if below + height <= bounds.bounds.size.height {
            below
        } else {
            (caret_y - height).max(px(0.))
        };
        let left = (bounds.cell_width * content.cursor.point.column as f32)
            .min((bounds.bounds.size.width - width).max(px(0.)));
        let foreground = self.settings.colors.terminal_foreground;
        let mut popup = div()
            .id("terminal-suggestions")
            .absolute()
            .left(left)
            .top(top)
            .w(width)
            .max_h(bounds.bounds.size.height)
            .overflow_hidden()
            .py_1()
            .border_1()
            .border_color(foreground.opacity(0.3))
            .rounded_md()
            .bg(self.settings.colors.terminal_background)
            .text_color(foreground)
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            });
        for (index, command) in self.suggestions.candidates.iter().enumerate() {
            let selected = index == self.suggestions.selected;
            let mut row = div()
                .id(("terminal-suggestion", index))
                .h(row_height)
                .px_3()
                .text_sm()
                .overflow_hidden()
                .cursor_pointer()
                .child(command.clone());
            if selected {
                row = row.bg(self.settings.selection_color);
            }
            popup = popup.child(row.on_click(cx.listener(move |this, _, window, cx| {
                if this.ime_state.is_none() {
                    this.suggestions.selected = index;
                    this.accept_suggestion(cx);
                    window.focus(&this.focus_handle, cx);
                }
            })));
        }
        Some(
            popup
                .child(
                    div()
                        .h(row_height)
                        .px_3()
                        .text_xs()
                        .text_color(foreground.opacity(0.65))
                        .child("命令建议 · ↑↓ 选择 · Tab 插入 · Esc 关闭"),
                )
                .into_any_element(),
        )
    }
}
