//! Types used by provider plugins.

pub use crate::types::{
    AssistantContent, Completion, CompletionRequest, FinishReason, Message, ProviderError,
    RateLimit, ToolCall, ToolDefinition, ToolResult, Usage,
};

/// Produces model completions for CHAP agent turns.
#[allow(async_fn_in_trait)]
pub trait Provider {
    /// Completes one request using this invocation's plugin state.
    async fn complete(&self, request: CompletionRequest) -> Result<Completion, ProviderError>;
}
