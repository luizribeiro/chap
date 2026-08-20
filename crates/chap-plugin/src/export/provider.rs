//! Types used by provider plugins.

pub use crate::types::{
    AssistantContent, Completion, CompletionRequest, FinishReason, Message, ToolCall,
    ToolDefinition, ToolResult,
};

/// Produces model completions for SAGE agent turns.
#[allow(async_fn_in_trait)]
pub trait Provider {
    /// Completes one request using this invocation's plugin state.
    async fn complete(&self, request: CompletionRequest) -> Result<Completion, String>;
}
