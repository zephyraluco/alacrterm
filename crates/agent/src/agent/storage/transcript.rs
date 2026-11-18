use crate::types::{Event, EventKind, TargetBinding};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum MessageState {
    Streaming,
    #[default]
    Complete,
    Cancelled,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct Message {
    pub run: Uuid,
    pub outgoing: bool,
    pub body: String,
    pub thinking: String,
    pub state: MessageState,
    pub tools: Vec<ToolTrace>,
}
#[derive(Debug, Clone)]
pub struct ToolTrace {
    pub id: String,
    pub name: String,
    pub input: Value,
    pub output: Option<String>,
    pub is_error: bool,
}
#[derive(Debug, Clone)]
pub struct ApprovalCard {
    pub id: Uuid,
    pub name: String,
    pub input: Value,
    pub target: Option<TargetBinding>,
}

/// A disposable UI projection. Native model memory is never built from it.
pub struct Transcript {
    pub session: Uuid,
    pub sequence: u64,
    pub active_run: Option<Uuid>,
    pub messages: Vec<Message>,
    pub approvals: Vec<ApprovalCard>,
}
impl Transcript {
    pub fn new(session: Uuid) -> Self {
        Self {
            session,
            sequence: 0,
            active_run: None,
            messages: vec![],
            approvals: vec![],
        }
    }

    /// Duplicate delivery is harmless. A gap must be replayed from the store.
    pub fn apply(&mut self, event: &Event) -> Result<bool, String> {
        if event.session != self.session {
            return Err("event belongs to another session".into());
        }
        if event.sequence <= self.sequence {
            return Ok(false);
        }
        if event.sequence != self.sequence + 1 {
            return Err("event sequence gap; reload the persisted transcript".into());
        }
        match &event.kind {
            EventKind::Started { prompt, .. } => {
                self.active_run = Some(event.run);
                self.approvals.clear();
                for outgoing in [true, false] {
                    self.messages.push(Message {
                        run: event.run,
                        outgoing,
                        body: if outgoing {
                            prompt.clone()
                        } else {
                            String::new()
                        },
                        thinking: String::new(),
                        state: if outgoing {
                            MessageState::Complete
                        } else {
                            MessageState::Streaming
                        },
                        tools: vec![],
                    });
                }
            }
            EventKind::ApprovalRequested {
                id,
                name,
                input,
                target,
            } => self.approvals.push(ApprovalCard {
                id: *id,
                name: name.clone(),
                input: input.clone(),
                target: target.clone(),
            }),
            EventKind::ApprovalResolved { id, .. } => self.approvals.retain(|card| card.id != *id),
            kind => {
                if let Some(message) = self
                    .messages
                    .iter_mut()
                    .rev()
                    .find(|message| message.run == event.run && !message.outgoing)
                {
                    match kind {
                        EventKind::TextDelta { text } => message.body.push_str(text),
                        EventKind::ThinkingDelta { text } => message.thinking.push_str(text),
                        EventKind::ToolStarted { id, name, input } => {
                            message.tools.push(ToolTrace {
                                id: id.clone(),
                                name: name.clone(),
                                input: input.clone(),
                                output: None,
                                is_error: false,
                            })
                        }
                        EventKind::ToolFinished {
                            id,
                            output,
                            is_error,
                        } => {
                            if let Some(tool) = message.tools.iter_mut().find(|tool| tool.id == *id)
                            {
                                tool.output = Some(output.clone());
                                tool.is_error = *is_error;
                            }
                        }
                        EventKind::Completed => message.state = MessageState::Complete,
                        EventKind::Cancelled => message.state = MessageState::Cancelled,
                        EventKind::Failed { message: error } => {
                            message.state = MessageState::Failed(error.clone())
                        }
                        _ => {}
                    }
                }
                if kind.is_terminal() && self.active_run == Some(event.run) {
                    self.active_run = None;
                    self.approvals.clear();
                }
            }
        }
        self.sequence = event.sequence;
        Ok(true)
    }

    pub fn title(&self) -> String {
        self.messages.first().map_or_else(
            || "新对话".into(),
            |m| {
                m.body
                    .lines()
                    .next()
                    .unwrap_or("新对话")
                    .chars()
                    .take(48)
                    .collect()
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Mode;
    #[test]
    fn replay_is_idempotent_and_requires_contiguous_session_events() {
        let id = Uuid::new_v4();
        let run = Uuid::new_v4();
        let mut transcript = Transcript::new(id);
        let mut event = Event {
            session: id,
            run,
            sequence: 1,
            kind: EventKind::Started {
                prompt: "hello".into(),
                target: None,
                mode: Mode::Ask,
            },
        };
        assert!(transcript.apply(&event).unwrap());
        assert!(!transcript.apply(&event).unwrap());
        event.sequence = 3;
        event.kind = EventKind::TextDelta {
            text: "response".into(),
        };
        assert!(transcript.apply(&event).is_err());
        event.sequence = 2;
        transcript.apply(&event).unwrap();
        event.sequence = 3;
        event.kind = EventKind::Failed {
            message: "connection lost".into(),
        };
        transcript.apply(&event).unwrap();
        assert_eq!(transcript.messages.len(), 2);
        assert_eq!(transcript.messages[1].body, "response");
        assert_eq!(
            transcript.messages[1].state,
            MessageState::Failed("connection lost".into())
        );
        assert!(transcript.active_run.is_none());
        assert_eq!(transcript.title(), "hello");
        event.session = Uuid::new_v4();
        assert!(transcript.apply(&event).is_err());
    }
}
