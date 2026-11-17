//! Host-provided tools only: do not register Solaris's local Bash toolset.
pub use solaris_tools::{
    Cancel, ExecutionMode, Tool, ToolContext, ToolOutput, ToolRegistry, ToolSelection,
};
