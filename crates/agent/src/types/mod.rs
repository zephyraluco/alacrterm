//! Stable application identities; never serialize GUI entity handles.
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
