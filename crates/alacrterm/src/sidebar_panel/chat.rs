//! UI-only conversation panel. No network requests or terminal command execution.
use crate::assets::{IconName, ui_icon};
mod composer;
use composer::{ComposerOptions, ContextAttachment, ModelProfile};
use gpui::{
    AppContext as _, ClipboardItem, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, WeakEntity, Window, div, px,
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _,
    bubble::{Bubble, BubbleVariant},
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{InputEvent, Textarea, TextareaState},
    list::ListItem,
    message::{Message, MessageAlignment, MessageContent, MessageFooter},
    message_scroller::{MessageScroller, MessageScrollerState},
    v_flex,
};
use std::time::{Duration, SystemTime};

#[derive(Clone)]
struct ChatMessage {
    body: SharedString,
    outgoing: bool,
    attempts: usize,
    attachments: Vec<ContextAttachment>,
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
            attachments: Vec::new(),
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ChatScreen {
    #[default]
    Sessions,
    Conversation(usize),
}

#[derive(Default)]
struct Navigation {
    current: Option<usize>,
    screen: ChatScreen,
}

impl Navigation {
    fn select(&mut self, index: usize) {
        self.current = Some(index);
        self.screen = ChatScreen::Conversation(index);
    }

    fn back(&mut self) {
        self.screen = ChatScreen::Sessions;
    }

    fn resume(&mut self) {
        self.screen = self
            .current
            .map_or(ChatScreen::Sessions, ChatScreen::Conversation);
    }
}

struct ChatSession {
    conversation: Conversation,
    options: ComposerOptions,
    updated: SystemTime,
    scroller: Entity<MessageScrollerState>,
    composer: Entity<TextareaState>,
    _subscriptions: Vec<Subscription>,
}

impl Conversation {
    fn title(&self) -> SharedString {
        self.messages.first().map_or_else(
            || "新对话".into(),
            |message| {
                message
                    .body
                    .lines()
                    .next()
                    .unwrap_or("新对话")
                    .chars()
                    .take(48)
                    .collect::<String>()
                    .into()
            },
        )
    }
}

fn relative_time(elapsed: Duration) -> String {
    match elapsed.as_secs() {
        0..60 => "刚刚".into(),
        seconds @ 60..3600 => format!("{} 分钟前", seconds / 60),
        seconds @ 3600..86400 => format!("{} 小时前", seconds / 3600),
        seconds => format!("{} 天前", seconds / 86400),
    }
}

pub(crate) struct ChatPanel {
    root: WeakEntity<crate::AppRoot>,
    models: Vec<ModelProfile>,
    sessions: Vec<ChatSession>,
    navigation: Navigation,
}

impl ChatPanel {
    pub(crate) fn new(
        root: WeakEntity<crate::AppRoot>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Self {
        Self {
            root,
            models: Vec::new(),
            sessions: Vec::new(),
            navigation: Navigation::default(),
        }
    }

    /// Opening the AI panel resumes the current conversation, when one exists.
    pub(crate) fn open(&mut self, cx: &mut Context<Self>) {
        self.navigation.resume();
        cx.notify();
    }

    fn new_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let index = self.sessions.len();
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
            move |this: &mut Self, _, event, window, cx| {
                if this.navigation.screen == ChatScreen::Conversation(index)
                    && matches!(
                        event,
                        InputEvent::PressEnter {
                            secondary: false,
                            shift: false
                        }
                    )
                {
                    this.send(window, cx);
                }
                cx.notify();
            },
        );
        let terminal = self.current_terminal(cx);
        self.sessions.push(ChatSession {
            conversation: Conversation::default(),
            options: ComposerOptions::with_terminal(terminal),
            updated: SystemTime::now(),
            scroller,
            composer,
            _subscriptions: vec![scroll_sub, input_sub],
        });
        self.select_session(index, window, cx);
    }

    fn select_session(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.sessions.get(index) else {
            return;
        };
        self.navigation.select(index);
        session
            .composer
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ChatScreen::Conversation(index) = self.navigation.screen else {
            return;
        };
        let Some(session) = self.sessions.get_mut(index) else {
            return;
        };
        let body = session.composer.read(cx).value();
        if !session.conversation.submit(&body) {
            return;
        }
        if let Some(message) = session.conversation.messages.last_mut() {
            message.attachments = std::mem::take(&mut session.options.attachments);
        }
        session.updated = SystemTime::now();
        session.composer.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        session.scroller.update(cx, |state, cx| {
            state.append(1, cx);
        });
        cx.notify();
    }

    fn render_sessions(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let mut order: Vec<_> = (0..self.sessions.len()).collect();
        order.sort_by_key(|&index| std::cmp::Reverse(self.sessions[index].updated));
        v_flex()
            .size_full()
            .min_h_0()
            .child(
                v_flex()
                    .id("chat-session-list")
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .children(self.sessions.is_empty().then(|| {
                        div()
                            .p_4()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("暂无对话，点击 + 开始。")
                    }))
                    .children(order.into_iter().map(|index| {
                        let session = &self.sessions[index];
                        let title = session.conversation.title();
                        let elapsed = session.updated.elapsed().unwrap_or_default();
                        ListItem::new(("chat-session", index))
                            .w_full()
                            .py_2()
                            .accessibility_label(title.clone())
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .w_full()
                                    .gap_1()
                                    .child(div().text_sm().truncate().child(title))
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .items_center()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(ui_icon(IconName::Bot))
                                            .child(format!(
                                                "{} 条消息 · {}",
                                                session.conversation.messages.len(),
                                                relative_time(elapsed)
                                            )),
                                    ),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_session(index, window, cx)
                            }))
                    })),
            )
            .into_any_element()
    }

    fn render_message(
        &self,
        session_index: usize,
        index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(message) = self
            .sessions
            .get(session_index)
            .and_then(|session| session.conversation.messages.get(index))
        else {
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
                            if let Some(session) = this.sessions.get_mut(session_index) {
                                session.conversation.retry(index);
                                session.updated = SystemTime::now();
                                session.scroller.update(cx, |state, cx| {
                                    state.remeasure_items(index..index + 1, cx);
                                });
                            }
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
                        .child(message.body.clone())
                        .children(
                            message
                                .attachments
                                .iter()
                                .map(|attachment| div().text_xs().child(attachment.label())),
                        ),
                ),
            )
            .footer(footer)
            .into_any_element()
    }
}

impl Render for ChatPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ChatScreen::Conversation(session_index) = self.navigation.screen else {
            return self.render_sessions(cx);
        };
        let Some(session) = self.sessions.get(session_index) else {
            return self.render_sessions(cx);
        };
        let scroller = session.scroller.clone();
        let panel = cx.weak_entity();
        v_flex()
            .size_full()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .child(
                MessageScroller::new(
                    ("chat-messages", session_index),
                    scroller,
                    move |index, _, cx| {
                        panel
                            .update(cx, |this, cx| {
                                this.render_message(session_index, index, cx)
                                    .into_any_element()
                            })
                            .unwrap_or_else(|_| div().into_any_element())
                    },
                )
                .flex_1()
                .min_h_0()
                .w_full()
                .with_jump_button_label("回到最新消息"),
            )
            .child(self.render_composer(session_index, cx))
            .into_any_element()
    }
}

impl crate::components::SidebarPanel for ChatPanel {
    const INFO: crate::components::PanelInfo = crate::components::PanelInfo {
        id: crate::components::PanelId("chat"),
        title: "AI 会话",
        icon: IconName::Bot,
        tooltip: "打开对话",
        default_width: px(360.),
    };
    fn on_open(&mut self, cx: &mut Context<Self>) {
        self.open(cx);
    }
    fn render_header(&mut self, _: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        match self.navigation.screen {
            ChatScreen::Sessions => h_flex()
                .w_full()
                .min_w_0()
                .items_center()
                .justify_between()
                .child(div().text_xs().child(Self::INFO.title))
                .child(
                    Button::new("chat-create")
                        .ghost()
                        .xsmall()
                        .icon(ui_icon(IconName::Plus))
                        .tooltip("新建对话")
                        .accessibility_label("新建对话")
                        .on_click(cx.listener(|this, _, window, cx| this.new_chat(window, cx))),
                )
                .into_any_element(),
            ChatScreen::Conversation(index) => {
                let title = self
                    .sessions
                    .get(index)
                    .map_or_else(|| "新对话".into(), |session| session.conversation.title());
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("chat-back")
                            .ghost()
                            .xsmall()
                            .icon(ui_icon(IconName::ArrowLeft))
                            .tooltip("返回会话列表")
                            .accessibility_label("返回会话列表")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.navigation.back();
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1().min_w_0().text_xs().truncate().child(title))
                    .into_any_element()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opening_panel_routes_to_sessions_or_resumes_current_conversation() {
        let mut navigation = Navigation::default();
        navigation.resume();
        assert_eq!(navigation.screen, ChatScreen::Sessions);
        navigation.select(0);
        navigation.back();
        assert_eq!(navigation.screen, ChatScreen::Sessions);
        assert_eq!(navigation.current, Some(0));
        navigation.resume();
        assert_eq!(navigation.screen, ChatScreen::Conversation(0));
        navigation.select(1);
        navigation.back();
        navigation.resume();
        assert_eq!(navigation.screen, ChatScreen::Conversation(1));
    }

    #[test]
    fn session_titles_and_relative_times_are_readable() {
        let mut conversation = Conversation::default();
        assert_eq!(conversation.title().as_ref(), "新对话");
        conversation.submit("删除历史schema文件\n其他内容");
        assert_eq!(conversation.title().as_ref(), "删除历史schema文件");
        conversation.submit("another message");
        assert_eq!(conversation.title().as_ref(), "删除历史schema文件");
        assert_eq!(relative_time(Duration::from_secs(0)), "刚刚");
        assert_eq!(relative_time(Duration::from_secs(8 * 3600)), "8 小时前");
        assert_eq!(relative_time(Duration::from_secs(4 * 86400)), "4 天前");
    }

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
            attachments: Vec::new(),
        });
        conversation.retry(1);
        assert_eq!(conversation.messages[1].attempts, 0);
    }
}
