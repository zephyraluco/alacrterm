//! Tools implemented by the application host.
mod filesystem;
mod host;
pub use filesystem::{
    DirectoryEntry, DirectoryListing, FileContent, FilesystemHost, filesystem_tools,
};
pub use host::{CommandResult, TerminalContext, TerminalHost, terminal_tools};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);
impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    Parallel,
    Sequential,
}
#[derive(Clone, Default)]
pub struct ToolContext {
    pub cancel: Cancel,
}
impl ToolContext {
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
}
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
}
impl ToolOutput {
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }
    pub fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }
}
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> serde_json::Value;
    fn is_read_only(&self) -> bool;
    fn execution_mode(&self) -> ExecutionMode {
        if self.is_read_only() {
            ExecutionMode::Parallel
        } else {
            ExecutionMode::Sequential
        }
    }
    async fn run(&self, input: serde_json::Value, context: &ToolContext) -> ToolOutput;
}
