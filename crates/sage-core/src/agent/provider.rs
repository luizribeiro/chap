use super::{AgentInner, PLUGIN_FUEL_PER_CALL, bindings};
use crate::{
    ToolDefinition,
    session::{AssistantContent, Message, ToolCall},
};
use bindings::provider as provider_bindings;
use lockgate::InvocationCtx;
use std::{future::Future, pin::Pin};

pub(super) type CompletionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ProviderCompletion, String>> + Send + 'a>>;

pub(super) trait CompletionBackend: Sync {
    fn complete(&self, messages: Vec<Message>) -> CompletionFuture<'_>;
}

pub(super) struct PluginBackend<'a> {
    runtime: &'a AgentInner,
    provider: &'a str,
}

impl<'a> PluginBackend<'a> {
    pub(super) fn new(runtime: &'a AgentInner, provider: &'a str) -> Self {
        Self { runtime, provider }
    }
}

impl CompletionBackend for PluginBackend<'_> {
    fn complete(&self, messages: Vec<Message>) -> CompletionFuture<'_> {
        Box::pin(self.runtime.request_completion(self.provider, messages))
    }
}

impl AgentInner {
    async fn request_completion(
        &self,
        provider: &str,
        messages: Vec<Message>,
    ) -> Result<ProviderCompletion, String> {
        let plugin = self
            .plugins
            .get(provider)
            .filter(|plugin| plugin.provider)
            .ok_or_else(|| format!("provider plugin `{provider}` is not configured"))?;
        self.lockgate
            .client::<provider_bindings::Role>(&plugin.handle)
            .map_err(|error| format!("provider plugin `{provider}` failed: {error}"))?
            .complete(
                InvocationCtx::bounded(PLUGIN_FUEL_PER_CALL),
                provider_bindings::CompletionRequest {
                    messages: messages.into_iter().map(Into::into).collect(),
                    tools: self
                        .tools
                        .definitions()
                        .into_iter()
                        .map(Into::into)
                        .collect(),
                },
            )
            .await
            .map_err(|error| format!("provider plugin `{provider}` failed: {error}"))?
            .map_err(|error| format!("provider plugin `{provider}`: {error}"))
            .map(Into::into)
    }
}

#[derive(Clone, Debug)]
pub(super) struct ProviderCompletion {
    pub(super) content: Vec<AssistantContent>,
}

impl From<Message> for provider_bindings::Message {
    fn from(message: Message) -> Self {
        match message {
            Message::System(content) => Self::System(content),
            Message::User(content) => Self::User(content),
            Message::Assistant(content) => {
                Self::Assistant(content.into_iter().map(Into::into).collect())
            }
            Message::ToolResult(result) => Self::ToolResult(provider_bindings::ToolResult {
                call_id: result.call_id,
                name: result.name,
                output: result.output,
                is_error: result.is_error,
            }),
        }
    }
}

impl From<AssistantContent> for provider_bindings::AssistantContent {
    fn from(content: AssistantContent) -> Self {
        match content {
            AssistantContent::Text(text) => Self::Text(text),
            AssistantContent::ToolCall(call) => Self::ToolCall(provider_bindings::ToolCall {
                id: call.id,
                name: call.name,
                arguments: call.arguments,
            }),
        }
    }
}

impl From<ToolDefinition> for provider_bindings::ToolDefinition {
    fn from(definition: ToolDefinition) -> Self {
        Self {
            name: definition.name,
            description: definition.description,
            parameters: definition.parameters,
        }
    }
}

impl From<provider_bindings::Completion> for ProviderCompletion {
    fn from(completion: provider_bindings::Completion) -> Self {
        Self {
            content: completion
                .content
                .into_iter()
                .map(|content| match content {
                    provider_bindings::AssistantContent::Text(text) => AssistantContent::Text(text),
                    provider_bindings::AssistantContent::ToolCall(call) => {
                        AssistantContent::ToolCall(ToolCall {
                            id: call.id,
                            name: call.name,
                            arguments: call.arguments,
                        })
                    }
                })
                .collect(),
        }
    }
}
