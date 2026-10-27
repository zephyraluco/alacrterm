//! Non-focusing, cursor-anchored terminal completion surface.
use crate::TerminalView;
use gpui::{
    AnyElement, Context, HighlightStyle, InteractiveElement as _, IntoElement as _, MouseButton,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, StyledText, div, point, px,
    size,
};
use gpui_kit::{
    base::Positioner,
    component::{
        ActiveTheme as _, Icon, IconName, list::ListItem, scroll::Scrollbar, v_virtual_list,
    },
};
use std::ops::Range;
use terminal::Modes;
impl TerminalView {
    pub(crate) fn render_suggestions(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.completion.popup_visible = false;
        if !self.focused
            || self.ime_state.is_some()
            || !self.completion.model.has_current_candidates()
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
        let width = bounds.bounds.size.width.min(px(480.)).max(px(0.));
        let row_height = px(24.);
        let caret_y = bounds.line_height * content.cursor.point.line.max(0) as f32;
        let below = (caret_y + bounds.line_height).min(bounds.bounds.size.height);
        let available_below = (bounds.bounds.size.height - below).max(px(0.));
        let available_above = caret_y.min(bounds.bounds.size.height).max(px(0.));
        let desired = row_height * self.completion.model.candidates.len().min(10) as f32 + px(2.);
        let place_below = desired <= available_below || available_below >= available_above;
        let available = if place_below {
            available_below
        } else {
            available_above
        };
        let visible_rows = (((available - px(2.)) / row_height).floor().max(0.) as usize)
            .min(10)
            .min(self.completion.model.candidates.len());
        if width < px(120.) || visible_rows == 0 {
            return None;
        }
        self.completion.popup_visible = true;
        let height = row_height * visible_rows as f32 + px(2.);
        let top = if place_below {
            below
        } else {
            (caret_y - height).max(px(0.))
        };
        // Keep following the end of the caret, including when the prefix grows.
        let left = (bounds.cell_width * content.cursor.point.column as f32)
            .min((bounds.bounds.size.width - width).max(px(0.)));
        let foreground = cx.theme().popover_foreground;
        let background = cx.theme().popover;
        let border = cx.theme().border;
        let font_size = self.settings.font_size.clamp(px(10.), px(16.));
        let item_sizes = std::rc::Rc::new(vec![
            size(width, row_height);
            self.completion.model.candidates.len()
        ]);
        let list = v_virtual_list(
            cx.entity(),
            "terminal-candidates",
            item_sizes,
            move |this, range, _, cx| {
                range
                    .filter_map(|index| {
                        let command = this.completion.model.candidates.get(index)?;
                        let mut detail = format!(
                            "{}\n{} · {}",
                            command.label,
                            command.kind.label(),
                            command.source.label()
                        );
                        if let Some(description) = &command.description {
                            detail.push('\n');
                            detail.push_str(description);
                        }
                        let highlights = highlight_ranges(
                            &command.label,
                            &this.completion.model.query_for(command),
                            this.completion.model.powershell == Some(true),
                            command.kind == crate::completion::Kind::Command
                                && this.completion.model.settings.fuzzy,
                        );
                        let mut content = div()
                            .flex()
                            .w_full()
                            .items_center()
                            .gap_2()
                            .overflow_hidden()
                            .child(
                                Icon::new(match command.kind {
                                    crate::completion::Kind::Directory => IconName::Folder,
                                    crate::completion::Kind::File => IconName::File,
                                    crate::completion::Kind::Command => IconName::SquareTerminal,
                                    _ => IconName::FileText,
                                })
                                .size_4()
                                .flex_shrink_0()
                                .text_color(foreground.opacity(0.65)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .line_height(px(20.))
                                    .truncate()
                                    .child(StyledText::new(command.label.clone()).with_highlights(
                                        highlights.into_iter().map(|range| {
                                            (
                                                range,
                                                HighlightStyle {
                                                    color: Some(cx.theme().blue),
                                                    ..Default::default()
                                                },
                                            )
                                        }),
                                    )),
                            );
                        if let Some(description) = &command.description {
                            content = content.child(
                                div()
                                    .min_w_0()
                                    .max_w(width * 0.45)
                                    .text_right()
                                    .text_color(foreground.opacity(0.55))
                                    .truncate()
                                    .child(description.clone()),
                            );
                        }
                        Some(
                            ListItem::new(("terminal-candidate", index))
                                .h(row_height)
                                .py_0()
                                .pl_2()
                                .pr(px(10.))
                                .w_full()
                                .font_family(this.settings.font_family.clone())
                                .text_size(font_size)
                                .line_height(px(20.))
                                // Preserve the component's current selected and hover styles.
                                .selected(index == this.completion.model.selected)
                                .tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(detail.clone())
                                        .build(window, cx)
                                })
                                .child(content)
                                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                    window.prevent_default();
                                    cx.stop_propagation();
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if this.ime_state.is_none() {
                                        this.completion.model.selected = index;
                                        this.accept_suggestion(cx);
                                        window.focus(&this.focus_handle, cx);
                                    }
                                })),
                        )
                    })
                    .collect::<Vec<_>>()
            },
        )
        .track_scroll(&self.suggestion_scroll)
        .size_full();
        let scrollbar = Scrollbar::vertical(&self.suggestion_scroll)
            .id("terminal-candidate-scrollbar")
            .styles(|styles| {
                styles
                    .track(|track| track.width(px(8.)))
                    .track_hover(|track| track.width(px(8.)))
                    .track_active(|track| track.width(px(8.)))
                    .thumb(|thumb| thumb.width(px(4.)).inset(px(2.)))
                    .thumb_hover(|thumb| thumb.width(px(6.)).inset(px(1.)))
                    .thumb_active(|thumb| thumb.width(px(6.)).inset(px(1.)))
            });
        let surface = div()
            .id("terminal-suggestions")
            .w(width)
            .h(height)
            .flex()
            .flex_col()
            .border_1()
            .border_color(border)
            .rounded(px(3.))
            .bg(background)
            .text_color(foreground)
            .overflow_hidden()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(list)
                    .child(scrollbar),
            );
        // Pane-local geometry prevents a deferred overlay spilling into another split.
        // Scrollbar geometry comes from the list handle and excludes the border.
        Some(
            gpui::deferred(
                Positioner::corner(
                    gpui::Anchor::TopLeft,
                    bounds.bounds.origin + point(left, top),
                )
                .margin(px(0.))
                .occlude()
                .child(surface),
            )
            .into_any_element(),
        )
    }
}

/// Use the same left-to-right subsequence as command scoring, and contiguous
/// matching for other providers. Map folded bytes back to the original label.
fn highlight_ranges(label: &str, query: &str, insensitive: bool, fuzzy: bool) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut folded = String::with_capacity(label.len());
    let mut original = Vec::with_capacity(label.len());
    for (start, ch) in label.char_indices() {
        let before = folded.len();
        if insensitive {
            folded.extend(ch.to_lowercase());
        } else {
            folded.push(ch);
        }
        original.extend(std::iter::repeat_n(
            start..start + ch.len_utf8(),
            folded.len() - before,
        ));
    }
    let query = if insensitive {
        query.to_lowercase()
    } else {
        query.into()
    };
    if let Some(start) = folded.find(&query)
        && (start == 0 || !fuzzy)
    {
        return std::iter::once(original[start].start..original[start + query.len() - 1].end)
            .collect();
    }
    if !fuzzy {
        return Vec::new();
    }
    let mut ranges: Vec<Range<usize>> = Vec::new();
    let mut at = 0;
    for ch in query.chars() {
        let Some(offset) = folded[at..].find(ch) else {
            return Vec::new();
        };
        let start = at + offset;
        at = start + ch.len_utf8();
        let range = original[start].start..original[at - 1].end;
        if let Some(previous) = ranges
            .last_mut()
            .filter(|previous| previous.end >= range.start)
        {
            previous.end = previous.end.max(range.end);
        } else {
            ranges.push(range);
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::highlight_ranges;

    #[test]
    fn highlights_prefixes_and_noncontiguous_command_characters() {
        assert_eq!(highlight_ranges("git", "gi", false, true), vec![0..2]);
        assert_eq!(highlight_ranges("git", "gt", false, true), vec![0..1, 2..3]);
        assert_eq!(
            highlight_ranges("gxxgt", "gt", false, true),
            vec![0..1, 4..5]
        );
        assert_eq!(
            highlight_ranges("cargo-clippy", "ccl", false, true),
            vec![0..1, 6..8]
        );
        assert!(highlight_ranges("git", "gx", false, true).is_empty());
        assert!(highlight_ranges("git", "gt", false, false).is_empty());
        assert!(highlight_ranges("git", "", false, true).is_empty());
    }

    #[test]
    fn highlights_use_shell_casing_and_original_utf8_offsets() {
        assert_eq!(
            highlight_ranges("Get-Item", "gt", true, true),
            vec![0..1, 2..3]
        );
        assert!(highlight_ranges("Get-Item", "gt", false, true).is_empty());
        assert_eq!(
            highlight_ranges("écho工具", "é工", false, true),
            vec![0..2, 5..8]
        );
        assert_eq!(highlight_ranges("İ-tool", "i-t", true, true), vec![0..4]);
        assert_eq!(highlight_ranges("İ-tool", "tool", true, false), vec![3..7]);
    }
}
