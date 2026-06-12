use gpui::{
    Action, App, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, Hsla, SharedString,
    Subscription, TaskExt, Window, actions, div, hsla, prelude::*, px,
};

use std::{rc::Rc, time::Duration};
actions!(
    menu,
    [
        /// Cancels the current menu operation.
        Cancel,
        /// Confirms the selected menu item.
        Confirm,
        /// Selects the previous item in the menu.
        SelectPrevious,
        /// Selects the next item in the menu.
        SelectNext,
        /// Selects the first item in the menu.
        SelectFirst,
        /// Selects the last item in the menu.
        SelectLast,
    ]
);

pub enum ContextMenuItem {
    Separator,
    Header(SharedString),
    Label(SharedString),
    Entry(ContextMenuEntry),
}

pub struct ContextMenuEntry {
    label: SharedString,
    handler: Rc<dyn Fn(Option<&FocusHandle>, &mut Window, &mut App)>,
    action: Option<Box<dyn Action>>,
    disabled: bool,
}

pub struct ContextMenu {
    items: Vec<ContextMenuItem>,
    focus_handle: FocusHandle,
    action_context: Option<FocusHandle>,
    selected_index: Option<usize>,
    delayed: bool,
    key_context: SharedString,
    _on_blur_subscription: Subscription,
}

impl Focusable for ContextMenu {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for ContextMenu {}

impl FluentBuilder for ContextMenu {}

impl ContextMenu {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(Self, &mut Window, &mut Context<Self>) -> Self,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let _on_blur_subscription = cx.on_blur(
            &focus_handle,
            window,
            |this: &mut ContextMenu, window, cx| this.cancel(&Cancel, window, cx),
        );
        window.refresh();

        f(
            Self {
                items: Default::default(),
                focus_handle,
                action_context: None,
                selected_index: None,
                delayed: false,
                key_context: "menu".into(),
                _on_blur_subscription,
            },
            window,
            cx,
        )
    }

    pub fn build(
        window: &mut Window,
        cx: &mut App,
        f: impl FnOnce(Self, &mut Window, &mut Context<Self>) -> Self,
    ) -> Entity<Self> {
        cx.new(|cx| Self::new(window, cx, f))
    }

    pub fn context(mut self, focus: FocusHandle) -> Self {
        self.action_context = Some(focus);
        self
    }

    pub fn header(mut self, title: impl Into<SharedString>) -> Self {
        self.items.push(ContextMenuItem::Header(title.into()));
        self
    }

    pub fn separator(mut self) -> Self {
        self.items.push(ContextMenuItem::Separator);
        self
    }

    pub fn entry(
        mut self,
        label: impl Into<SharedString>,
        action: Option<Box<dyn Action>>,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push(ContextMenuItem::Entry(ContextMenuEntry {
            label: label.into(),
            handler: Rc::new(move |_, window, cx| handler(window, cx)),
            action,
            disabled: false,
        }));
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.items.push(ContextMenuItem::Label(label.into()));
        self
    }

    pub fn action(self, label: impl Into<SharedString>, action: Box<dyn Action>) -> Self {
        self.action_disabled_when(false, label, action)
    }

    pub fn action_disabled_when(
        mut self,
        disabled: bool,
        label: impl Into<SharedString>,
        action: Box<dyn Action>,
    ) -> Self {
        self.items.push(ContextMenuItem::Entry(ContextMenuEntry {
            label: label.into(),
            action: Some(action.boxed_clone()),
            handler: Rc::new(move |context, window, cx| {
                if let Some(context) = &context {
                    window.focus(context, cx);
                }
                window.dispatch_action(action.boxed_clone(), cx);
            }),
            disabled,
        }));
        self
    }

    pub fn key_context(mut self, context: impl Into<SharedString>) -> Self {
        self.key_context = context.into();
        self
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.selected_index
    }

    pub fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.selected_index else {
            return;
        };

        let context = self.action_context.as_ref();

        if let Some(ContextMenuItem::Entry(ContextMenuEntry {
            handler,
            disabled: false,
            ..
        })) = self.items.get(ix)
        {
            (handler)(context, window, cx)
        }

        cx.emit(DismissEvent);
    }

    pub fn cancel(&mut self, _: &Cancel, _window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }

    pub fn clear_selected(&mut self) {
        self.selected_index = None;
    }

    pub fn select_first(&mut self, _: &SelectFirst, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.items.iter().position(|item| item.is_selectable()) {
            self.select_index(ix);
        }
        cx.notify();
    }

    pub fn select_last(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<usize> {
        for (ix, item) in self.items.iter().enumerate().rev() {
            if item.is_selectable() {
                return self.select_index(ix);
            }
        }
        None
    }

    fn handle_select_last(&mut self, _: &SelectLast, window: &mut Window, cx: &mut Context<Self>) {
        if self.select_last(window, cx).is_some() {
            cx.notify();
        }
    }

    pub fn select_next(&mut self, _: &SelectNext, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.selected_index {
            let next_index = ix + 1;
            if self.items.len() <= next_index {
                self.select_first(&SelectFirst, window, cx);
                return;
            } else {
                for (ix, item) in self.items.iter().enumerate().skip(next_index) {
                    if item.is_selectable() {
                        self.select_index(ix);
                        cx.notify();
                        return;
                    }
                }
            }
        }
        self.select_first(&SelectFirst, window, cx);
    }

    pub fn select_previous(
        &mut self,
        _: &SelectPrevious,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ix) = self.selected_index {
            for (ix, item) in self.items.iter().enumerate().take(ix).rev() {
                if item.is_selectable() {
                    self.select_index(ix);
                    cx.notify();
                    return;
                }
            }
        }
        self.handle_select_last(&SelectLast, window, cx);
    }

    fn select_index(&mut self, ix: usize) -> Option<usize> {
        let item = self.items.get(ix)?;
        if item.is_selectable() {
            self.selected_index = Some(ix);
        }
        Some(ix)
    }

    pub fn on_action_dispatch(
        &mut self,
        dispatched: &dyn Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ix) = self.items.iter().position(|item| {
            if let ContextMenuItem::Entry(ContextMenuEntry {
                action: Some(action),
                disabled: false,
                ..
            }) = item
            {
                action.partial_eq(dispatched)
            } else {
                false
            }
        }) {
            self.select_index(ix);
            self.delayed = true;
            cx.notify();
            let action = dispatched.boxed_clone();
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                cx.update(|window, cx| {
                    this.update(cx, |this, cx| {
                        this.cancel(&Cancel, window, cx);
                        window.dispatch_action(action, cx);
                    })
                })
            })
            .detach_and_log_err(cx);
        } else {
            cx.propagate()
        }
    }

    fn render_menu_item(
        &self,
        ix: usize,
        item: &ContextMenuItem,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        match item {
            ContextMenuItem::Separator => div()
                .h(px(1.))
                .mx_2()
                .my_1()
                .bg(Self::separator_color())
                .into_any_element(),
            ContextMenuItem::Header(header) => div()
                .px_3()
                .py_1()
                .text_xs()
                .text_color(Self::muted_text_color())
                .child(header.clone())
                .into_any_element(),
            ContextMenuItem::Label(label) => div()
                .px_3()
                .py_1()
                .text_sm()
                .text_color(Self::disabled_text_color())
                .child(label.clone())
                .into_any_element(),
            ContextMenuItem::Entry(entry) => {
                self.render_menu_entry(ix, entry, cx).into_any_element()
            }
        }
    }

    fn render_menu_entry(
        &self,
        ix: usize,
        entry: &ContextMenuEntry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ContextMenuEntry {
            label,
            handler,
            action,
            disabled,
        } = entry;

        let handler = handler.clone();
        let menu = cx.entity().downgrade();

        let label_color = if *disabled {
            Self::disabled_text_color()
        } else {
            Self::text_color()
        };

        div()
            .id(("context-menu-child", ix))
            .px_3()
            .py_1()
            .when(Some(ix) == self.selected_index, |el| {
                el.bg(Self::selected_color())
            })
            .when(!*disabled, |item| {
                item.cursor_pointer()
                    .hover(|style| style.bg(Self::selected_color()))
                    .on_mouse_move(cx.listener(move |this, _, window, cx| {
                        this.clear_selected();
                        window.focus(&this.focus_handle.clone(), cx);
                    }))
            })
            .child(
                div()
                    .flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(
                        div()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_sm()
                            .text_color(label_color)
                            .child(label.clone()),
                    )
                    .children(action.as_ref().map(|_| div().ml_4())),
            )
            .when(!*disabled, |item| {
                item.on_click({
                    let context = self.action_context.clone();
                    move |_, window, cx| {
                        handler(context.as_ref(), window, cx);
                        menu.update(cx, |_, cx| {
                            cx.emit(DismissEvent);
                        })
                        .ok();
                    }
                })
            })
            .into_any_element()
    }

    fn text_color() -> Hsla {
        hsla(0., 0., 0.92, 1.)
    }

    fn muted_text_color() -> Hsla {
        hsla(0., 0., 0.62, 1.)
    }

    fn disabled_text_color() -> Hsla {
        hsla(0., 0., 0.42, 1.)
    }

    fn separator_color() -> Hsla {
        hsla(0., 0., 0.24, 1.)
    }

    fn selected_color() -> Hsla {
        hsla(0., 0., 0.22, 1.)
    }
}

impl ContextMenuItem {
    fn is_selectable(&self) -> bool {
        match self {
            ContextMenuItem::Header(_)
            | ContextMenuItem::Separator
            | ContextMenuItem::Label { .. } => false,
            ContextMenuItem::Entry(ContextMenuEntry { disabled, .. }) => !disabled,
        }
    }
}

impl Render for ContextMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .flex_shrink_0()
            .bg(hsla(0., 0., 0.12, 1.))
            .border_1()
            .border_color(hsla(0., 0., 0.28, 1.))
            .rounded_md()
            .shadow_lg()
            .occlude()
            .child(
                div()
                    .id("context-menu")
                    .flex()
                    .flex_col()
                    .max_h(window.viewport_size().height * 0.75)
                    .flex_shrink_0()
                    .min_w(px(200.))
                    .flex_1()
                    .overflow_y_scroll()
                    .track_focus(&self.focus_handle(cx))
                    .key_context(self.key_context.as_ref())
                    .on_action(cx.listener(ContextMenu::select_first))
                    .on_action(cx.listener(ContextMenu::handle_select_last))
                    .on_action(cx.listener(ContextMenu::select_next))
                    .on_action(cx.listener(ContextMenu::select_previous))
                    .on_action(cx.listener(ContextMenu::confirm))
                    .on_action(cx.listener(ContextMenu::cancel))
                    .on_mouse_down_out(
                        cx.listener(|this, _, window, cx| this.cancel(&Cancel, window, cx)),
                    )
                    .when(!self.delayed, |mut el| {
                        for item in self.items.iter() {
                            if let ContextMenuItem::Entry(ContextMenuEntry {
                                action: Some(action),
                                disabled: false,
                                ..
                            }) = item
                            {
                                el = el.on_boxed_action(
                                    &**action,
                                    cx.listener(ContextMenu::on_action_dispatch),
                                );
                            }
                        }
                        el
                    })
                    .children(
                        self.items
                            .iter()
                            .enumerate()
                            .map(|(ix, item)| self.render_menu_item(ix, item, cx)),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    #[gpui::test]
    fn can_navigate_back_over_headers(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let context_menu = cx.update(|window, cx| {
            ContextMenu::build(window, cx, |menu, _, _| {
                menu.header("First header")
                    .separator()
                    .entry("First entry", None, |_, _| {})
                    .separator()
                    .separator()
                    .entry("Last entry", None, |_, _| {})
                    .header("Last header")
            })
        });

        context_menu.update_in(cx, |context_menu, window, cx| {
            assert_eq!(
                None, context_menu.selected_index,
                "No selection is in the menu initially"
            );

            context_menu.select_first(&SelectFirst, window, cx);
            assert_eq!(
                Some(2),
                context_menu.selected_index,
                "Should select first selectable entry, skipping the header and the separator"
            );

            context_menu.select_next(&SelectNext, window, cx);
            assert_eq!(
                Some(5),
                context_menu.selected_index,
                "Should select next selectable entry, skipping 2 separators along the way"
            );

            context_menu.select_next(&SelectNext, window, cx);
            assert_eq!(
                Some(2),
                context_menu.selected_index,
                "Should wrap around to first selectable entry"
            );
        });

        context_menu.update_in(cx, |context_menu, window, cx| {
            assert_eq!(
                Some(2),
                context_menu.selected_index,
                "Should start from the first selectable entry"
            );

            context_menu.select_previous(&SelectPrevious, window, cx);
            assert_eq!(
                Some(5),
                context_menu.selected_index,
                "Should wrap around to previous selectable entry (last)"
            );

            context_menu.select_previous(&SelectPrevious, window, cx);
            assert_eq!(
                Some(2),
                context_menu.selected_index,
                "Should go back to previous selectable entry (first)"
            );
        });

        context_menu.update_in(cx, |context_menu, window, cx| {
            context_menu.select_first(&SelectFirst, window, cx);
            assert_eq!(
                Some(2),
                context_menu.selected_index,
                "Should start from the first selectable entry"
            );

            context_menu.select_previous(&SelectPrevious, window, cx);
            assert_eq!(
                Some(5),
                context_menu.selected_index,
                "Should wrap around to last selectable entry"
            );
            context_menu.select_next(&SelectNext, window, cx);
            assert_eq!(
                Some(2),
                context_menu.selected_index,
                "Should wrap around to first selectable entry"
            );
        });
    }
}
