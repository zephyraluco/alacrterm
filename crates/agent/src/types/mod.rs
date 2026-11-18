//! Stable application identities; never serialize GUI entity handles.
pub type SessionId = uuid::Uuid;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TerminalId(pub uuid::Uuid);
impl Default for TerminalId {
    fn default() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TargetBinding {
    pub terminal: TerminalId,
    pub generation: u64,
    pub label: String,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Ask,
    Agent,
}
pub use solaris_types::{ModelCapabilities, Support, ToolCall, ToolResult, ToolSpec};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: uuid::Uuid,
    pub title: String,
    pub model: Option<uuid::Uuid>,
    pub target: Option<TargetBinding>,
    pub updated_ms: u64,
    pub active_run: Option<uuid::Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub session: uuid::Uuid,
    pub run: uuid::Uuid,
    pub sequence: u64,
    pub kind: EventKind,
}

#[derive(Debug, Clone)]
pub struct RunInput {
    pub model: uuid::Uuid,
    pub target: Option<TargetBinding>,
    pub prompt: String,
    pub mode: Mode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    Started {
        prompt: String,
        target: Option<TargetBinding>,
        mode: Mode,
    },
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        text: String,
    },
    ToolStarted {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolFinished {
        id: String,
        output: String,
        is_error: bool,
    },
    ApprovalRequested {
        id: uuid::Uuid,
        name: String,
        input: serde_json::Value,
        target: Option<TargetBinding>,
    },
    ApprovalResolved {
        id: uuid::Uuid,
        approved: bool,
    },
    Completed,
    Cancelled,
    Failed {
        message: String,
    },
}

impl EventKind {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Cancelled | Self::Failed { .. }
        )
    }
}
