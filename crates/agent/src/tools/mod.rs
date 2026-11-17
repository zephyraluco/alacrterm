//! Host-provided tools only: do not register Solaris's local Bash toolset.
mod host;
pub use host::{CommandResult, TerminalContext, TerminalHost, terminal_tools};
pub use solaris_tools::{
    Cancel, ExecutionMode, Tool, ToolContext, ToolOutput, ToolRegistry, ToolSelection,
};
