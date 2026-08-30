//! Types used by tool plugins.

pub use crate::types::{ExecutionMode, ToolDefinition};

/// A failure reported while executing a tool.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolError {
    /// The arguments were malformed or invalid; the model is expected to retry.
    InvalidInput(String),
    /// Consent or policy refused the call.
    Denied(String),
    /// The tool ran and failed; fed back to the model.
    Failed(String),
    /// The plugin or its environment is broken; the host aborts the turn.
    Fatal(String),
}

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
    async fn execute(&self, name: String, arguments: String) -> Result<String, ToolError>;
}
