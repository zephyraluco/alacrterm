//! Chat UI projects persisted events; the agent service owns model/tool execution.
use crate::assets::{IconName, ui_icon};
mod composer;
use agent::{
    agent::{
        runtime::{AgentService, StartRun},
        storage::{MessageState, Transcript},
    },
    provider::{CredentialSource, ModelProfile},
    types::{Event, Session, TerminalId},
};
use composer::ComposerOptions;
use gpui::{
    AppContext as _, ClipboardItem, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, WeakEntity, Window, div, prelude::FluentBuilder as _, px,
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
    text::TextView,
    v_flex,
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

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

fn relative_time(elapsed: Duration) -> String {
    match elapsed.as_secs() {
        0..60 => "刚刚".into(),
        seconds @ 60..3600 => format!("{} 分钟前", seconds / 60),
        seconds @ 3600..86400 => format!("{} 小时前", seconds / 3600),
        seconds => format!("{} 天前", seconds / 86400),
    }
}

struct ChatSession {
    id: agent::types::SessionId,
    transcript: Transcript,
    options: ComposerOptions,
    updated: SystemTime,
    scroller: Entity<MessageScrollerState>,
    composer: Entity<TextareaState>,
    preparing: bool,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}
impl ChatSession {
    fn busy(&self) -> bool {
        self.preparing || self.transcript.active_run.is_some()
    }
}

pub(crate) struct ChatPanel {
    root: WeakEntity<crate::AppRoot>,
    service: Result<Arc<AgentService>, String>,
    models: Vec<ModelProfile>,
    model_reasoning: HashMap<agent::types::SessionId, Vec<String>>,
    configuration_error: Option<String>,
    sessions: Vec<ChatSession>,
    navigation: Navigation,
    expanded_tools: HashSet<(agent::types::SessionId, usize)>,
    terminal_ids: HashMap<gpui::EntityId, TerminalId>,
    _events: gpui::Task<()>,
}
impl ChatPanel {
    pub(crate) fn new(
        root: WeakEntity<crate::AppRoot>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let service = crate::agent_host::service(cx);
        let profiles = crate::agent_host::profiles();
        let configuration_error = profiles.as_ref().err().map(ToString::to_string);
        let models = profiles.unwrap_or_default();
        let model_reasoning = models
            .iter()
            .map(|model| (model.id, model.reasoning_levels()))
            .collect();
        let mut panel = Self {
            root,
            service,
            models,
            model_reasoning,
            configuration_error,
            sessions: vec![],
            navigation: Navigation::default(),
            expanded_tools: HashSet::new(),
            terminal_ids: HashMap::new(),
            _events: gpui::Task::ready(()),
        };
        if let Ok(service) = panel.service.clone() {
            let mut receiver = service.subscribe();
            match service.sessions() {
                Ok(sessions) => {
                    for session in sessions {
                        panel.add_session(session, window, cx);
                    }
                }
                Err(error) => panel.configuration_error = Some(error.to_string()),
            }
            panel._events = cx.spawn(async move |this, cx| {
                loop {
                    match receiver.recv().await {
                        Ok(event) => {
                            if this
                                .update(cx, |this, cx| this.apply_event(event, cx))
                                .is_err()
                            {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            let replay = service.sessions().and_then(|sessions| {
                                sessions
                                    .into_iter()
                                    .map(|s| Ok((s.id, service.events(s.id, 0)?)))
                                    .collect::<anyhow::Result<Vec<_>>>()
                            });
                            if this
                                .update(cx, |this, cx| {
                                    match replay {
                                        Ok(sessions) => {
                                            for (id, events) in sessions {
                                                if let Some(index) =
                                                    this.sessions.iter().position(|s| s.id == id)
                                                {
                                                    for event in events {
                                                        this.apply_event(event, cx);
                                                    }
                                                    this.sessions[index].preparing = false;
                                                }
                                            }
                                        }
                                        Err(error) => {
                                            this.configuration_error = Some(error.to_string())
                                        }
                                    }
                                    cx.notify();
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        }
        panel
    }

    pub(crate) fn open(&mut self, cx: &mut Context<Self>) {
        self.navigation.resume();
        cx.notify();
    }

    fn add_session(&mut self, metadata: Session, window: &mut Window, cx: &mut Context<Self>) {
        let index = self.sessions.len();
        let mut transcript = Transcript::new(metadata.id);
        if let Ok(service) = &self.service {
            match service.events(metadata.id, 0) {
                Ok(events) => {
                    for event in events {
                        if let Err(error) = transcript.apply(&event) {
                            self.configuration_error = Some(error);
                            break;
                        }
                    }
                }
                Err(error) => self.configuration_error = Some(error.to_string()),
            }
        }
        let scroller = cx.new(|cx| MessageScrollerState::new(transcript.messages.len(), cx));
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
        let mut options = ComposerOptions::with_terminal(None);
        options.model = metadata
            .model
            .and_then(|id| self.models.iter().position(|m| m.id == id));
        self.sessions.push(ChatSession {
            id: metadata.id,
            transcript,
            options,
            updated: UNIX_EPOCH + Duration::from_millis(metadata.updated_ms),
            scroller,
            composer,
            preparing: false,
            error: None,
            _subscriptions: vec![scroll_sub, input_sub],
        });
    }

    fn new_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let result = self
            .service
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|service| service.create_session().map_err(|e| e.to_string()));
        match result {
            Ok(session) => {
                let index = self.sessions.len();
                self.add_session(session, window, cx);
                self.sessions[index].options.terminal = self.current_terminal(cx);
                if !self.models.is_empty() {
                    self.sessions[index].options.model = Some(0);
                }
                self.select_session(index, window, cx);
            }
            Err(error) => self.report_error(error, cx),
        }
    }

    fn report_error(&self, error: String, cx: &mut Context<Self>) {
        let _ = self.root.update(cx, |root, cx| {
            root.notify(
                gpui_kit::component::notification::Notification::error(error),
                cx,
            )
        });
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

    fn apply_event(&mut self, event: Event, cx: &mut Context<Self>) {
        let Some(session) = self.sessions.iter_mut().find(|s| s.id == event.session) else {
            return;
        };
        let before = session.transcript.messages.len();
        match session.transcript.apply(&event) {
            Ok(true) => {
                session.updated = SystemTime::now();
                let after = session.transcript.messages.len();
                session.scroller.update(cx, |state, cx| {
                    if after > before {
                        state.append(after - before, cx);
                    }
                    if after > 0 {
                        state.remeasure_items(after - 1..after, cx);
                    }
                });
            }
            Ok(false) => {}
            Err(error) => session.error = Some(error.into()),
        }
        cx.notify();
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ChatScreen::Conversation(index) = self.navigation.screen else {
            return;
        };
        let Some(session) = self.sessions.get(index) else {
            return;
        };
        if session.busy() {
            return;
        }
        let body = session.composer.read(cx).value().trim().to_owned();
        if body.is_empty() {
            return;
        }
        let Some(model) = session
            .options
            .model
            .and_then(|id| self.models.get(id))
            .cloned()
        else {
            self.report_error("请先配置并选择模型".into(), cx);
            return;
        };
        let service = match &self.service {
            Ok(service) => service.clone(),
            Err(error) => {
                self.report_error(error.clone(), cx);
                return;
            }
        };
        let mut tools = vec![];
        let mode = session.options.mode.core();
        let reasoning = session
            .options
            .reasoning_for(
                self.model_reasoning
                    .get(&model.id)
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
            )
            .map(|value| value.wire().to_owned());
        let selected = session.options.terminal;
        let attachments = session.options.attachments.clone();
        let id = session.id;
        let mut target = None;
        if let Some(selected) = selected {
            let view = self
                .root
                .read_with(cx, |root, cx| {
                    root.tabs.iter().find_map(|tab| {
                        tab.read(cx)
                            .views()
                            .iter()
                            .find(|v| v.entity_id() == selected)
                            .cloned()
                    })
                })
                .ok()
                .flatten();
            let Some(view) = view else {
                self.report_error("所选终端已关闭，请重新选择".into(), cx);
                return;
            };
            let terminal_id = *self.terminal_ids.entry(selected).or_default();
            let label = view.read(cx).title().to_string();
            match crate::agent_host::bind_terminal(&view, terminal_id, label, mode, cx) {
                Ok(bound) => {
                    tools = bound.tools;
                    target = Some(bound.target);
                }
                Err(error) => {
                    self.report_error(error, cx);
                    return;
                }
            }
        }
        let (instructions, mcp) = match composer::resources(&attachments) {
            Ok(resources) => resources,
            Err(error) => {
                self.report_error(error, cx);
                return;
            }
        };
        let credential = model.credential.clone();
        let credential_task = if let CredentialSource::Keychain { account } = &credential {
            Some(cx.read_credentials(&crate::agent_host::credential_url(account)))
        } else {
            None
        };
        self.sessions[index].preparing = true;
        self.sessions[index].error = None;
        cx.spawn_in(window, async move |this, cx| {
            let secret = match credential {
                CredentialSource::None => Ok(None),
                CredentialSource::Environment { variable } => std::env::var(variable)
                    .map(Some)
                    .map_err(|_| "API Key 环境变量不存在或无法读取".to_owned()),
                CredentialSource::Keychain { .. } => match credential_task {
                    Some(task) => match task.await {
                        Ok(Some((_, bytes))) => String::from_utf8(bytes)
                            .map(Some)
                            .map_err(|_| "API Key 编码错误".to_owned()),
                        Ok(None) => Err("系统凭据库中未找到 API Key，请重新配置模型".into()),
                        Err(_) => Err("无法访问系统凭据库，请使用环境变量或检查系统钥匙串".into()),
                    },
                    None => Err("凭据请求不可用".into()),
                },
            };
            let result = match secret {
                Ok(secret) => {
                    cx.background_spawn(async move {
                        service
                            .start(StartRun {
                                session: id,
                                model,
                                secret,
                                prompt: body,
                                mode,
                                target,
                                reasoning,
                                tools,
                                instructions,
                                mcp,
                            })
                            .map_err(|e| e.to_string())
                    })
                    .await
                }
                Err(error) => Err(error),
            };
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(session) = this.sessions.get_mut(index) else {
                    return;
                };
                session.preparing = false;
                match result {
                    Ok(_) => {
                        session
                            .composer
                            .update(cx, |input, cx| input.set_value("", window, cx));
                    }
                    Err(error) => session.error = Some(error.into()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn stop(&mut self, index: usize, cx: &mut Context<Self>) {
        if let (Ok(service), Some(session)) = (&self.service, self.sessions.get(index)) {
            service.cancel(session.id);
        }
        cx.notify();
    }
    fn render_sessions(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let mut order: Vec<_> = (0..self.sessions.len()).collect();
        order.sort_by_key(|&index| std::cmp::Reverse(self.sessions[index].updated));
        v_flex()
            .size_full()
            .min_h_0()
            .children(
                self.configuration_error
                    .as_ref()
                    .or_else(|| self.service.as_ref().err())
                    .map(|error| {
                        div()
                            .p_3()
                            .text_xs()
                            .text_color(cx.theme().danger)
                            .child(error.clone())
                    }),
            )
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
                        let title = session.transcript.title();
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
                                                session.transcript.messages.len(),
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
    ) -> gpui::AnyElement {
        let Some(message) = self
            .sessions
            .get(session_index)
            .and_then(|s| s.transcript.messages.get(index))
        else {
            return div().into_any_element();
        };
        let body = message.body.clone();
        let mut footer = MessageFooter::new().child(
            Button::new(("chat-copy", index))
                .ghost()
                .xsmall()
                .icon(ui_icon(IconName::Copy))
                .tooltip("复制消息")
                .accessibility_label("复制消息")
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(body.clone()))
                }),
        );
        let status = match &message.state {
            MessageState::Streaming => Some("正在生成…".to_owned()),
            MessageState::Cancelled => Some("已停止".to_owned()),
            MessageState::Failed(error) => Some(error.clone()),
            MessageState::Complete => None,
        };
        if let Some(status) = status {
            footer = footer.child(
                div()
                    .text_xs()
                    .text_color(if matches!(message.state, MessageState::Failed(_)) {
                        cx.theme().danger
                    } else {
                        cx.theme().muted_foreground
                    })
                    .child(status),
            );
        }
        if !message.outgoing
            && matches!(
                message.state,
                MessageState::Failed(_) | MessageState::Cancelled
            )
        {
            let run = message.run;
            footer = footer.child(
                Button::new(("chat-retry", index))
                    .ghost()
                    .xsmall()
                    .icon(ui_icon(IconName::Refresh))
                    .tooltip("重新发送这条请求")
                    .accessibility_label("重新发送请求")
                    .disabled(self.sessions[session_index].busy())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let session = &this.sessions[session_index];
                        let body = session
                            .transcript
                            .messages
                            .iter()
                            .find(|m| m.run == run && m.outgoing)
                            .map(|m| m.body.clone());
                        if let Some(body) = body {
                            session
                                .composer
                                .update(cx, |input, cx| input.set_value(body, window, cx));
                            this.navigation.select(session_index);
                            this.send(window, cx);
                        }
                    })),
            );
        }
        let mut content = v_flex().min_w_0().gap_2();
        if message.outgoing {
            content = content.child(message.body.clone());
        } else {
            if !message.thinking.is_empty() {
                content = content.child(
                    div()
                        .id(("chat-thinking", index))
                        .max_h(px(96.))
                        .overflow_y_scroll()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(message.thinking.clone()),
                );
            }
            if !message.tools.is_empty() {
                content = content.child(
                    v_flex()
                        .w_full()
                        .border_l_1()
                        .border_color(cx.theme().border)
                        .pl_2()
                        .children(message.tools.iter().enumerate().map(|(tool_index, tool)| {
                            let key = (message.run, tool_index);
                            let expanded = self.expanded_tools.contains(&key);
                            let status = match (&tool.output, &message.state) {
                                (Some(_), _) if tool.is_error => "失败",
                                (Some(_), _) => "完成",
                                (None, MessageState::Streaming) => "运行中",
                                (None, _) => "未完成",
                            };
                            let row = Button::new(("chat-tool-toggle", tool_index))
                                .ghost()
                                .xsmall()
                                .icon(ui_icon(if expanded {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                }))
                                .label(format!("{} · {status}", tool.name))
                                .max_w_full()
                                .overflow_hidden()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if !this.expanded_tools.remove(&key) {
                                        this.expanded_tools.insert(key);
                                    }
                                    this.sessions[session_index].scroller.update(
                                        cx,
                                        |state, cx| {
                                            state.remeasure_items(index..index + 1, cx);
                                        },
                                    );
                                    cx.notify();
                                }));
                            v_flex()
                                .w_full()
                                .min_w_0()
                                .items_start()
                                .py_1()
                                .child(row)
                                .when(expanded, |details| {
                                    details.child(
                                        div()
                                            .id(("chat-tool-details", tool_index))
                                            .w_full()
                                            .max_h(px(180.))
                                            .overflow_y_scroll()
                                            .rounded_sm()
                                            .bg(cx.theme().muted)
                                            .p_2()
                                            .text_xs()
                                            .child(
                                                serde_json::to_string_pretty(&tool.input)
                                                    .unwrap_or_default(),
                                            )
                                            .children(
                                                tool.output
                                                    .iter()
                                                    .cloned()
                                                    .map(|output| div().mt_2().child(output)),
                                            ),
                                    )
                                })
                        })),
                );
            }
            if !message.body.is_empty() {
                content = content.child(
                    TextView::markdown(("chat-markdown", index), message.body.clone())
                        .selectable(true)
                        .on_link_click(|url, _, _, cx| {
                            if url.starts_with("https://") || url.starts_with("http://") {
                                cx.open_url(url);
                            }
                        })
                        .image_source(|_| {
                            gpui::ImageSource::Custom(std::sync::Arc::new(|_, _| None))
                        }),
                );
            }
        }

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
                        .child(content),
                ),
            )
            .footer(footer)
            .into_any_element()
    }

    fn render_approvals(&self, index: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let session = &self.sessions[index];
        v_flex()
            .w_full()
            .gap_2()
            .px_3()
            .children(
                session
                    .transcript
                    .approvals
                    .iter()
                    .enumerate()
                    .map(|(i, card)| {
                        let allow = self.service.as_ref().ok().cloned();
                        let deny = allow.clone();
                        let id = card.id;
                        v_flex()
                            .p_2()
                            .gap_2()
                            .border_1()
                            .border_color(cx.theme().border)
                            .rounded_md()
                            .child(format!("允许 {}？", card.name))
                            .children(
                                card.target
                                    .as_ref()
                                    .map(|t| div().text_xs().child(format!("终端：{}", t.label))),
                            )
                            .child(
                                div()
                                    .id(("chat-approval-details", i))
                                    .max_h(px(160.))
                                    .overflow_y_scroll()
                                    .text_xs()
                                    .child(
                                        serde_json::to_string_pretty(&card.input)
                                            .unwrap_or_default(),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .justify_end()
                                    .child(
                                        Button::new(("chat-deny", i))
                                            .small()
                                            .label("拒绝")
                                            .on_click(move |_, _, _| {
                                                if let Some(service) = &deny {
                                                    service.approve(id, false);
                                                }
                                            }),
                                    )
                                    .child(
                                        Button::new(("chat-allow", i))
                                            .small()
                                            .primary()
                                            .label("允许本次")
                                            .on_click(move |_, _, _| {
                                                if let Some(service) = &allow {
                                                    service.approve(id, true);
                                                }
                                            }),
                                    ),
                            )
                    }),
            )
            .into_any_element()
    }
}

impl Render for ChatPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ChatScreen::Conversation(index) = self.navigation.screen else {
            return self.render_sessions(cx);
        };
        let Some(session) = self.sessions.get(index) else {
            return self.render_sessions(cx);
        };
        let scroller = session.scroller.clone();
        let error = session.error.clone();
        let panel = cx.weak_entity();
        v_flex()
            .size_full()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .child(
                MessageScroller::new(("chat-messages", index), scroller, move |message, _, cx| {
                    panel
                        .update(cx, |this, cx| this.render_message(index, message, cx))
                        .unwrap_or_else(|_| div().into_any_element())
                })
                .flex_1()
                .min_h_0()
                .w_full()
                .with_jump_button_label("回到最新消息"),
            )
            .children(error.map(|error| {
                div()
                    .px_3()
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(error)
            }))
            .child(self.render_approvals(index, cx))
            .child(self.render_composer(index, cx))
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
                    .map_or_else(|| "新对话".into(), |session| session.transcript.title());
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
        let mut nav = Navigation::default();
        nav.resume();
        assert_eq!(nav.screen, ChatScreen::Sessions);
        nav.select(0);
        nav.back();
        assert_eq!(nav.screen, ChatScreen::Sessions);
        nav.resume();
        assert_eq!(nav.screen, ChatScreen::Conversation(0));
    }
    #[test]
    fn relative_times_are_readable() {
        assert_eq!(relative_time(Duration::from_secs(0)), "刚刚");
        assert_eq!(relative_time(Duration::from_secs(8 * 3600)), "8 小时前");
    }
}
