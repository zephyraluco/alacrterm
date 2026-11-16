//! UI-only conversation panel. No network requests or terminal command execution.
use crate::assets::{IconName, ui_icon};
use gpui::{
    AppContext as _, ClipboardItem, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div, px,
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _,
    bubble::{Bubble, BubbleVariant},
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{InputEvent, Textarea, TextareaState},
    message::{Message, MessageAlignment, MessageContent, MessageFooter},
    message_scroller::{MessageScroller, MessageScrollerState},
    v_flex,
};

#[derive(Clone)]
struct ChatMessage {
    body: SharedString,
    outgoing: bool,
    attempts: usize,
}

#[derive(Default)]
struct Conversation {
    messages: Vec<ChatMessage>,
}

impl Conversation {
    fn submit(&mut self, body: &str) -> bool {
        let body = body.trim();
        if body.is_empty() {
            return false;
        }
        self.messages.push(ChatMessage {
            body: body.to_owned().into(),
            outgoing: true,
            attempts: 1,
        });
        true
    }

    fn retry(&mut self, index: usize) {
        if let Some(message) = self
            .messages
            .get_mut(index)
            .filter(|message| message.outgoing)
        {
            // The UI has no transport yet: a retry must never claim delivery.
            message.attempts = message.attempts.saturating_add(1);
        }
    }
}

pub(crate) struct ChatPanel {
    conversation: Conversation,
    scroller: Entity<MessageScrollerState>,
    composer: Entity<TextareaState>,
    _subscriptions: Vec<Subscription>,
}

impl ChatPanel {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let conversation = Conversation::default();
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 6)
                .submit_on_enter(true)
                .placeholder("输入消息…")
        });
        let scroll_sub = cx.observe(&scroller, |_, _, cx| cx.notify());
        let input_sub = cx.subscribe_in(
            &composer,
            window,
            |this: &mut Self, _, event, window, cx| {
                if matches!(
                    event,
                    InputEvent::PressEnter {
                        secondary: false,
                        shift: false
                    }
                ) {
                    this.send(window, cx);
                }
                cx.notify();
            },
        );
        Self {
            conversation,
            scroller,
            composer,
            _subscriptions: vec![scroll_sub, input_sub],
        }
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let body = self.composer.read(cx).value();
        if !self.conversation.submit(&body) {
            return;
        }
        self.composer.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        self.scroller.update(cx, |state, cx| {
            state.append(1, cx);
        });
        cx.notify();
    }

    fn new_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.conversation.messages.clear();
        self.scroller.update(cx, |state, cx| state.reset(0, cx));
        self.composer.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    fn render_message(&self, index: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(message) = self.conversation.messages.get(index) else {
            return div().into_any_element();
        };
        let body = message.body.clone();
        let copy = Button::new(("chat-copy", index))
            .ghost()
            .xsmall()
            .icon(ui_icon(IconName::Copy))
            .tooltip("复制消息")
            .accessibility_label("复制消息")
            .on_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(body.to_string()))
            });
        let footer = if message.outgoing {
            let attempts = message.attempts;
            MessageFooter::new()
                .gap_2()
                .child(div().text_color(cx.theme().danger).child(if attempts == 1 {
                    "发送失败".to_owned()
                } else {
                    format!("重试失败（{}）", attempts - 1)
                }))
                .child(
                    Button::new(("chat-retry", index))
                        .ghost()
                        .xsmall()
                        .icon(ui_icon(IconName::Refresh))
                        .accessibility_label("重试发送")
                        .tooltip("重试发送（尚未连接对话服务）")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.conversation.retry(index);
                            this.scroller.update(cx, |state, cx| {
                                state.remeasure_items(index..index + 1, cx);
                            });
                            cx.notify();
                        })),
                )
                .child(copy)
        } else {
            MessageFooter::new().child(copy)
        };
        Message::new()
            .id(("chat-message", index))
            .alignment(if message.outgoing {
                MessageAlignment::End
            } else {
                MessageAlignment::Start
            })
            .content(
                MessageContent::new().bubble(
                    Bubble::new()
                        .with_variant(if message.outgoing {
                            BubbleVariant::Muted
                        } else {
                            BubbleVariant::Ghost
                        })
                        .child(message.body.clone()),
                ),
            )
            .footer(footer)
            .into_any_element()
    }
}

impl Render for ChatPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let panel = cx.weak_entity();
        let empty = self.composer.read(cx).value().trim().is_empty();
        v_flex()
            .size_full()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .child(
                h_flex()
                    .w_full()
                    .flex_none()
                    .items_start()
                    .justify_between()
                    .p_4()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        v_flex()
                            .min_w_0()
                            .gap_1()
                            .child(div().text_base().child("新对话"))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("今天有什么可以帮你？"),
                            ),
                    )
                    .child(
                        Button::new("chat-new")
                            .outline()
                            .small()
                            .size(px(32.))
                            .rounded(px(16.))
                            .icon(ui_icon(IconName::Refresh))
                            .tooltip("新建对话")
                            .accessibility_label("新建对话")
                            .on_click(cx.listener(|this, _, window, cx| this.new_chat(window, cx))),
                    ),
            )
            .child(
                MessageScroller::new(
                    "chat-messages",
                    self.scroller.clone(),
                    move |index, _, cx| {
                        panel
                            .update(cx, |this, cx| {
                                this.render_message(index, cx).into_any_element()
                            })
                            .unwrap_or_else(|_| div().into_any_element())
                    },
                )
                .flex_1()
                .min_h_0()
                .w_full()
                .with_jump_button_label("回到最新消息"),
            )
            .child(
                div().flex_none().w_full().p_4().child(
                    v_flex()
                        .w_full()
                        .gap_1()
                        .p_3()
                        .rounded_xl()
                        .bg(cx.theme().muted)
                        .child(
                            Textarea::new(&self.composer)
                                .appearance(false)
                                .bordered(false)
                                .w_full(),
                        )
                        .child(
                            h_flex().w_full().justify_end().child(
                                Button::new("chat-send")
                                    .small()
                                    .size(px(32.))
                                    .bg(cx.theme().foreground)
                                    .text_color(cx.theme().background)
                                    .rounded(px(16.))
                                    .icon(ui_icon(IconName::ArrowUp))
                                    .tooltip("发送（Enter），换行（Shift+Enter）")
                                    .accessibility_label("发送消息")
                                    .disabled(empty)
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.send(window, cx)),
                                    ),
                            ),
                        ),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sending_and_retrying_without_transport_never_duplicate_or_claim_delivery() {
        let mut conversation = Conversation::default();
        assert!(!conversation.submit(" \n "));
        assert!(conversation.submit(" hello 世界 "));
        assert_eq!(conversation.messages[0].body.as_ref(), "hello 世界");
        conversation.retry(0);
        conversation.retry(99);
        assert_eq!(conversation.messages.len(), 1);
        assert_eq!(conversation.messages[0].attempts, 2);
        assert!(conversation.messages[0].outgoing);
        conversation.messages.push(ChatMessage {
            body: "reply".into(),
            outgoing: false,
            attempts: 0,
        });
        conversation.retry(1);
        assert_eq!(conversation.messages[1].attempts, 0);
    }
}
