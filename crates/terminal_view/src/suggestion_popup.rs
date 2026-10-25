//! Non-focusing, virtualized terminal completion surface.
use crate::TerminalView;
use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement as _, MouseButton,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, div, point, px, size,
};
use gpui_kit::{
    base::Positioner,
    component::{Icon, IconName, kbd::Kbd, list::ListItem, scroll::Scrollbar, v_virtual_list},
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
        let width = bounds.bounds.size.width.min(px(480.)).max(px(0.));
        let row_height = |candidate: &crate::completion::Candidate| {
            if candidate.description.is_some() {
                px(44.)
            } else {
                px(28.)
            }
        };
        let footer = px(28.);
        let caret_y = bounds.line_height * content.cursor.point.line.max(0) as f32;
        let below = (caret_y + bounds.line_height).min(bounds.bounds.size.height);
        let available_below = (bounds.bounds.size.height - below).max(px(0.));
        let available_above = caret_y.min(bounds.bounds.size.height).max(px(0.));
        let desired = self
            .suggestions
            .candidates
            .iter()
            .take(8)
            .fold(footer + px(2.), |height, candidate| {
                height + row_height(candidate)
            });
        let place_below = desired <= available_below || available_below >= available_above;
        let height = desired.min(if place_below {
            available_below
        } else {
            available_above
        });
        if width < px(120.)
            || height < row_height(&self.suggestions.candidates[0]) + footer + px(2.)
        {
            return None;
        }
        let top = if place_below {
            below
        } else {
            (caret_y - height).max(px(0.))
        };
        let left = (bounds.cell_width * content.cursor.point.column as f32)
            .min((bounds.bounds.size.width - width).max(px(0.)));
        let foreground = self.settings.colors.terminal_foreground;
        let item_sizes = std::rc::Rc::new(
            self.suggestions
                .candidates
                .iter()
                .map(|candidate| size(width, row_height(candidate)))
                .collect(),
        );
        let list = v_virtual_list(
            cx.entity(),
            "terminal-candidates",
            item_sizes,
            move |this, range, _, cx| {
                range
                    .filter_map(|index| {
                        let command = this.suggestions.candidates.get(index)?;
                        let detail = format!(
                            "{}\n{} · {}",
                            command.label,
                            command.kind.label(),
                            command.source.label()
                        );
                        let mut text = div()
                            .flex_1()
                            .min_w_0()
                            .line_height(px(20.))
                            .overflow_hidden()
                            .child(
                                div().truncate().child(
                                    gpui_kit::component::label::Label::new(command.label.clone())
                                        .highlights(
                                            gpui_kit::component::label::HighlightsMatch::Full(
                                                this.suggestions.query_for(command).into(),
                                            ),
                                        ),
                                ),
                            );
                        if let Some(description) = &command.description {
                            text = text.child(
                                div()
                                    .text_xs()
                                    .line_height(px(16.))
                                    .text_color(foreground.opacity(0.65))
                                    .truncate()
                                    .child(description.clone()),
                            );
                        }
                        Some(
                            ListItem::new(("terminal-candidate", index))
                                .h(row_height(command))
                                .py(px(2.))
                                .w_full()
                                .px_3()
                                .text_sm()
                                .selected(index == this.suggestions.selected)
                                .tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(detail.clone())
                                        .build(window, cx)
                                })
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            Icon::new(match command.kind {
                                                crate::completion::Kind::Directory => {
                                                    IconName::Folder
                                                }
                                                crate::completion::Kind::File => IconName::File,
                                                crate::completion::Kind::Command => {
                                                    IconName::SquareTerminal
                                                }
                                                _ => IconName::FileText,
                                            })
                                            .size_4()
                                            .text_color(foreground.opacity(0.65)),
                                        )
                                        .child(text),
                                )
                                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                    window.prevent_default();
                                    cx.stop_propagation();
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if this.ime_state.is_none() {
                                        this.suggestions.selected = index;
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
        let mut key_hints = div()
            .flex()
            .items_center()
            .gap_1()
            .h(footer)
            .flex_shrink_0()
            .px_3()
            .text_xs()
            .overflow_hidden()
            .text_color(foreground.opacity(0.65))
            .child(format!("{} 项", self.suggestions.candidates.len()));
        for (key, label) in [
            ("up", ""),
            ("down", "选择"),
            ("tab", "插入"),
            ("escape", "关闭"),
        ] {
            if width < px(340.) && matches!(key, "up" | "down") {
                continue;
            }
            if let Ok(stroke) = gpui::Keystroke::parse(key) {
                key_hints = key_hints
                    .child(Kbd::new(stroke).appearance(false))
                    .child(label);
            }
        }
        let surface = div()
            .id("terminal-suggestions")
            .w(width)
            .h(height)
            .flex()
            .flex_col()
            .border_1()
            .border_color(foreground.opacity(0.3))
            .rounded_md()
            .bg(self.settings.colors.terminal_background)
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
                    // Use the virtual list's measured viewport, not the scrollbar's
                    // absolute static position after the list in this block container.
                    .child(
                        Scrollbar::vertical(&self.suggestion_scroll)
                            .id("terminal-candidate-scrollbar"),
                    ),
            )
            .child(key_hints);
        // Pane-local geometry above prevents a window-positioned surface spilling into
        // a neighboring split. Positioner supplies deferred overlay layout and clamping.
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
