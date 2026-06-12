use std::sync::Arc;

use crate::TerminalView;
use gpui::{
    Action, App, AppContext as _, Context, Div, Entity, Focusable, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Render, Styled, Subscription, Task, WeakEntity, Window, actions,
    div,
};
use gpui_component::{
    ActiveTheme, h_flex,
    input::{Input, InputEvent, InputState},
};

pub const SEARCH_BAR_KEY_CONTEXT: &str = "AlacrtermSearchBar";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerminalSearchDirection {
    Prev,
    #[default]
    Next,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalSearchOptions {
    pub regex: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalSearchQuery {
    text: String,
    regex: bool,
}

impl TerminalSearchQuery {
    pub fn new(text: impl Into<String>, regex: bool) -> Self {
        Self {
            text: text.into(),
            regex,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn is_regex(&self) -> bool {
        self.regex
    }
}

pub trait TerminalSearchable {
    type Match: Send + Sync + Clone;

    fn supported_options(&self) -> TerminalSearchOptions;
    fn clear_matches(&mut self, window: &mut Window, cx: &mut Context<Self>)
    where
        Self: Sized;
    fn update_matches(
        &mut self,
        matches: &[Self::Match],
        active_match_index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        Self: Sized;
    fn query_suggestion(&mut self, window: &mut Window, cx: &mut Context<Self>) -> String
    where
        Self: Sized;
    fn activate_match(
        &mut self,
        index: usize,
        matches: &[Self::Match],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        Self: Sized;
    fn select_matches(
        &mut self,
        matches: &[Self::Match],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        Self: Sized;
    fn find_matches(
        &mut self,
        query: Arc<TerminalSearchQuery>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Vec<Self::Match>>
    where
        Self: Sized;
    fn active_match_index(
        &mut self,
        direction: TerminalSearchDirection,
        matches: &[Self::Match],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize>
    where
        Self: Sized;
}

actions!(
    terminal,
    [
        /// Toggles the terminal search bar.
        ToggleTerminalSearch,
        /// Dismisses the terminal search bar.
        DismissTerminalSearch,
        /// Selects the next terminal search match.
        SelectNextSearchMatch,
        /// Selects the previous terminal search match.
        SelectPreviousSearchMatch,
        /// Toggles regex mode for terminal search.
        ToggleTerminalSearchRegex,
    ]
);

pub struct TerminalSearchBar {
    terminal_view: WeakEntity<TerminalView>,
    input: Entity<InputState>,
    visible: bool,
    regex: bool,
    matches: Vec<terminal::Range>,
    active_match: Option<usize>,
    generation: u64,
    _subscriptions: Vec<Subscription>,
}

impl TerminalSearchBar {
    pub fn new(
        terminal_view: WeakEntity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find in terminal"));
        let input_subscription = cx.subscribe_in(&input, window, {
            move |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => this.refresh_search(window, cx),
                InputEvent::PressEnter { shift, .. } => {
                    if *shift {
                        this.select_previous_match(window, cx);
                    } else {
                        this.select_next_match(window, cx);
                    }
                }
                _ => {}
            }
        });

        Self {
            terminal_view,
            input,
            visible: false,
            regex: false,
            matches: Vec::new(),
            active_match: None,
            generation: 0,
            _subscriptions: vec![input_subscription],
        }
    }

    pub(crate) fn toggle_search(
        &mut self,
        _: &ToggleTerminalSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show(window, cx);
    }

    pub(crate) fn dismiss_search(
        &mut self,
        _: &DismissTerminalSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.visible {
            cx.propagate();
            return;
        }

        self.visible = false;
        self.matches.clear();
        self.active_match = None;

        if let Some(terminal_view) = self.terminal_view.upgrade() {
            terminal_view.update(cx, |terminal_view, cx| {
                terminal_view.clear_matches(window, cx);
            });
            window.focus(&terminal_view.read(cx).focus_handle(cx), cx);
        }
        cx.notify();
    }

    pub(crate) fn toggle_regex(
        &mut self,
        _: &ToggleTerminalSearchRegex,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.visible {
            cx.propagate();
            return;
        }
        self.regex = !self.regex;
        self.refresh_search(window, cx);
        cx.notify();
    }

    pub(crate) fn select_next_match_action(
        &mut self,
        _: &SelectNextSearchMatch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_next_match(window, cx);
    }

    pub(crate) fn select_previous_match_action(
        &mut self,
        _: &SelectPreviousSearchMatch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_previous_match(window, cx);
    }

    fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.visible = true;

        if let Some(terminal_view) = self.terminal_view.upgrade() {
            let suggestion = terminal_view.update(cx, |terminal_view, cx| {
                terminal_view.query_suggestion(window, cx)
            });
            if !suggestion.is_empty() {
                self.input
                    .update(cx, |input, cx| input.set_value(suggestion, window, cx));
            }
        }

        window.focus(&self.input.read(cx).focus_handle(cx), cx);
        self.refresh_search(window, cx);
        cx.notify();
    }

    fn refresh_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }

        let Some(terminal_view) = self.terminal_view.upgrade() else {
            return;
        };

        let query = self.input.read(cx).value().to_string();
        if query.is_empty() {
            self.matches.clear();
            self.active_match = None;
            terminal_view.update(cx, |terminal_view, cx| {
                terminal_view.clear_matches(window, cx);
            });
            cx.notify();
            return;
        }

        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let query = Arc::new(TerminalSearchQuery::new(query, self.regex));
        let matches_task = terminal_view.update(cx, |terminal_view, cx| {
            terminal_view.find_matches(query, window, cx)
        });
        let search_bar = cx.entity();

        cx.spawn_in(window, async move |_, cx| {
            let matches = matches_task.await;
            search_bar
                .update_in(cx, |search_bar, window, cx| {
                    if generation != search_bar.generation {
                        return;
                    }

                    let Some(terminal_view) = search_bar.terminal_view.upgrade() else {
                        return;
                    };

                    search_bar.matches = matches;
                    let matches = search_bar.matches.clone();
                    search_bar.active_match = terminal_view.update(cx, |terminal_view, cx| {
                        terminal_view.active_match_index(
                            TerminalSearchDirection::Next,
                            &matches,
                            window,
                            cx,
                        )
                    });

                    terminal_view.update(cx, |terminal_view, cx| {
                        terminal_view.update_matches(&matches, search_bar.active_match, window, cx);
                        if let Some(index) = search_bar.active_match {
                            terminal_view.activate_match(index, &matches, window, cx);
                        }
                    });
                    cx.notify();
                })
                .ok();
        })
        .detach();
    }

    fn select_next_match(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.select_match(TerminalSearchDirection::Next, window, cx);
    }

    fn select_previous_match(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.select_match(TerminalSearchDirection::Prev, window, cx);
    }

    fn select_match(
        &mut self,
        direction: TerminalSearchDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.visible || self.matches.is_empty() {
            cx.propagate();
            return;
        }

        let current = self.active_match.unwrap_or(match direction {
            TerminalSearchDirection::Prev => 0,
            TerminalSearchDirection::Next => self.matches.len().saturating_sub(1),
        });
        let next = match direction {
            TerminalSearchDirection::Prev => {
                if current == 0 {
                    self.matches.len().saturating_sub(1)
                } else {
                    current - 1
                }
            }
            TerminalSearchDirection::Next => (current + 1) % self.matches.len(),
        };
        self.active_match = Some(next);

        if let Some(terminal_view) = self.terminal_view.upgrade() {
            let matches = self.matches.clone();
            terminal_view.update(cx, |terminal_view, cx| {
                terminal_view.update_matches(&matches, self.active_match, window, cx);
                terminal_view.activate_match(next, &matches, window, cx);
            });
        }
        cx.notify();
    }

    fn match_status(&self, cx: &App) -> String {
        if self.input.read(cx).value().is_empty() {
            String::new()
        } else if let Some(active_match) = self.active_match {
            format!("{} / {}", active_match + 1, self.matches.len())
        } else {
            "No results".to_string()
        }
    }
}

impl Render for TerminalSearchBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let root = div()
            .on_action(cx.listener(Self::toggle_search))
            .on_action(cx.listener(Self::dismiss_search))
            .on_action(cx.listener(Self::toggle_regex))
            .on_action(cx.listener(Self::select_next_match_action))
            .on_action(cx.listener(Self::select_previous_match_action));

        if !self.visible {
            return root;
        }

        let colors = &cx.theme().colors;
        root.child(
            h_flex()
                .key_context(SEARCH_BAR_KEY_CONTEXT)
                .w_full()
                .gap_2()
                .items_center()
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(colors.border)
                .bg(colors.popover)
                .child(div().w_64().child(Input::new(&self.input)))
                .child(
                    search_button(
                        if self.regex { ".* on" } else { ".*" },
                        ToggleTerminalSearchRegex,
                    )
                    .border_color(if self.regex {
                        colors.primary
                    } else {
                        colors.border
                    }),
                )
                .child(search_button("Prev", SelectPreviousSearchMatch).border_color(colors.border))
                .child(search_button("Next", SelectNextSearchMatch).border_color(colors.border))
                .child(
                    div()
                        .min_w_20()
                        .text_sm()
                        .text_color(colors.muted_foreground)
                        .child(self.match_status(cx)),
                )
                .child(search_button("Close", DismissTerminalSearch).border_color(colors.border)),
        )
    }
}

fn search_button<A: Action + Clone + 'static>(label: &'static str, action: A) -> Div {
    div()
        .px_2()
        .py_1()
        .rounded_md()
        .border_1()
        .child(label)
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.dispatch_action(Box::new(action.clone()), cx);
        })
}
