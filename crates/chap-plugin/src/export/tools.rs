//! Types used by tool plugins.

pub use crate::types::{ExecutionMode, ToolDefinition};

/// Supplies tools that a CHAP agent can discover and execute.
#[allow(async_fn_in_trait)]
pub trait Tools {
    /// Lists the tools provided by this plugin.
    fn definitions(&self) -> Result<Vec<ToolDefinition>, String>;

    /// Declares whether a tool may run concurrently with other tool calls.
    fn execution_mode(&self, _name: &str) -> ExecutionMode {
        ExecutionMode::Parallel
    }

    /// Executes a named tool with JSON-encoded arguments.
    async fn execute(&self, name: String, arguments: String) -> Result<String, String>;
}
