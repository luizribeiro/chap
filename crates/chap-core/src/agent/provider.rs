use super::{AgentInner, bindings, telemetry::trace_plugin_call};
use crate::{
    FinishReason, ProviderError, ToolDefinition,
    session::{AssistantContent, Message, Reasoning, ToolCall, Usage},
};
use bindings::provider as provider_bindings;
use bindings::types as provider_types;
use lockgate::{CallError, PluginId};
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

pub(super) type CompletionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ProviderCompletion, ProviderError>> + Send + 'a>>;

pub(super) trait CompletionBackend: Sync {
    fn complete(&self, messages: Vec<Message>) -> CompletionFuture<'_>;
}

pub(super) struct PluginBackend<'a> {
    runtime: &'a AgentInner,
    provider: &'a PluginId,
}

impl<'a> PluginBackend<'a> {
    pub(super) fn new(runtime: &'a AgentInner, provider: &'a PluginId) -> Self {
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
        provider: &PluginId,
        messages: Vec<Message>,
    ) -> Result<ProviderCompletion, ProviderError> {
        let plugin = self
            .plugins
            .get(provider)
            .filter(|plugin| plugin.has_role(&chap_wit::PROVIDER))
            .ok_or_else(|| ProviderError::NotConfigured {
                provider: provider.clone(),
            })?;
        trace_plugin_call(provider.as_str(), "provider", "complete", async {
            self.lockgate
                .client::<provider_bindings::Role>(&plugin.handle)
                .map_err(|source| ProviderError::RoleUnavailable {
                    provider: provider.clone(),
                    source,
                })?
                .complete(provider_types::CompletionRequest {
                    messages: messages.into_iter().map(Into::into).collect(),
                    tools: self
                        .tools
                        .definitions()
                        .into_iter()
                        .map(Into::into)
                        .collect(),
                })
                .await
                .map_err(|error| map_plugin_call_error(provider, error))?
                .map_err(|error| map_provider_error(provider, error))
                .map(Into::into)
        })
        .await
    }
}

fn map_plugin_call_error(provider: &PluginId, error: CallError) -> ProviderError {
    match error {
        source @ CallError::DeadlineExceeded { deadline } => ProviderError::TimedOut {
            provider: provider.clone(),
            deadline,
            source: Arc::new(source),
        },
        source @ CallError::HostPanic { .. } => ProviderError::CallFailed {
            provider: provider.clone(),
            source: Arc::new(source),
        },
        source => ProviderError::CallFailed {
            provider: provider.clone(),
            source: Arc::new(source),
        },
    }
}

fn map_provider_error(provider: &PluginId, error: provider_types::ProviderError) -> ProviderError {
    match error {
        provider_types::ProviderError::RateLimited(error) => ProviderError::RateLimited {
            retry_after: error.retry_after.map(Duration::from_secs),
            message: provider_message(provider, error.message),
        },
        provider_types::ProviderError::ContextTooLong(message) => {
            ProviderError::ContextTooLong(provider_message(provider, message))
        }
        provider_types::ProviderError::Unauthorized(message) => {
            ProviderError::Unauthorized(provider_message(provider, message))
        }
        provider_types::ProviderError::Unavailable(message) => {
            ProviderError::Unavailable(provider_message(provider, message))
        }
        provider_types::ProviderError::Refused(message) => {
            ProviderError::Refused(provider_message(provider, message))
        }
        provider_types::ProviderError::Other(message) => {
            ProviderError::Other(provider_message(provider, message))
        }
    }
}

fn provider_message(provider: &PluginId, message: String) -> String {
    format!("provider plugin `{provider}`: {message}")
}

#[derive(Clone, Debug)]
pub(super) struct ProviderCompletion {
    pub(super) content: Vec<AssistantContent>,
    pub(super) finish_reason: FinishReason,
    pub(super) usage: Option<Usage>,
}

impl From<Message> for provider_types::Message {
    fn from(message: Message) -> Self {
        match message {
            Message::System(content) => Self::System(content),
            Message::User(content) => Self::User(content),
            Message::Assistant(content) => {
                Self::Assistant(content.into_iter().map(Into::into).collect())
            }
            Message::ToolResult(result) => Self::ToolResult(provider_types::ToolResult {
                call_id: result.call_id,
                name: result.name,
                output: result.output,
                is_error: result.is_error,
            }),
        }
    }
}

impl From<AssistantContent> for provider_types::AssistantContent {
    fn from(content: AssistantContent) -> Self {
        match content {
            AssistantContent::Text(text) => Self::Text(text),
            AssistantContent::Reasoning(reasoning) => Self::Reasoning(provider_types::Reasoning {
                text: reasoning.text,
                signature: reasoning.signature,
            }),
            AssistantContent::ToolCall(call) => Self::ToolCall(provider_types::ToolCall {
                id: call.id,
                name: call.name,
                arguments: call.arguments,
            }),
        }
    }
}

impl From<ToolDefinition> for provider_types::ToolDefinition {
    fn from(definition: ToolDefinition) -> Self {
        Self {
            name: definition.name,
            description: definition.description,
            parameters: definition.parameters,
        }
    }
}

impl From<provider_types::Completion> for ProviderCompletion {
    fn from(completion: provider_types::Completion) -> Self {
        Self {
            content: completion
                .content
                .into_iter()
                .map(|content| match content {
                    provider_types::AssistantContent::Text(text) => AssistantContent::Text(text),
                    provider_types::AssistantContent::Reasoning(reasoning) => {
                        AssistantContent::Reasoning(Reasoning {
                            text: reasoning.text,
                            signature: reasoning.signature,
                        })
                    }
                    provider_types::AssistantContent::ToolCall(call) => {
                        AssistantContent::ToolCall(ToolCall {
                            id: call.id,
                            name: call.name,
                            arguments: call.arguments,
                        })
                    }
                })
                .collect(),
            finish_reason: match completion.finish_reason {
                provider_types::FinishReason::Stop => FinishReason::Stop,
                provider_types::FinishReason::ToolCalls => FinishReason::ToolCalls,
                provider_types::FinishReason::Length => FinishReason::Length,
                provider_types::FinishReason::Other(reason) => FinishReason::Other(reason),
            },
            usage: completion.usage.map(|usage| Usage {
                input_tokens: usage.input_tokens,
                cached_input_tokens: usage.cached_input_tokens,
                cache_write_tokens: usage.cache_write_tokens,
                output_tokens: usage.output_tokens,
                reasoning_tokens: usage.reasoning_tokens,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_error_implements_std_error() {
        fn assert_error(_: &dyn std::error::Error) {}

        assert_error(&ProviderError::Other("provider failed".to_owned()));
    }

    #[test]
    fn maps_each_provider_error() {
        let provider = PluginId::from("example");
        let errors = [
            (
                provider_types::ProviderError::RateLimited(provider_types::RateLimit {
                    retry_after: Some(30),
                    message: "slow down".to_owned(),
                }),
                ProviderError::RateLimited {
                    retry_after: Some(Duration::from_secs(30)),
                    message: "provider plugin `example`: slow down".to_owned(),
                },
                "provider plugin `example`: slow down",
            ),
            (
                provider_types::ProviderError::RateLimited(provider_types::RateLimit {
                    retry_after: None,
                    message: "slow down".to_owned(),
                }),
                ProviderError::RateLimited {
                    retry_after: None,
                    message: "provider plugin `example`: slow down".to_owned(),
                },
                "provider plugin `example`: slow down",
            ),
            (
                provider_types::ProviderError::ContextTooLong("too many tokens".to_owned()),
                ProviderError::ContextTooLong(
                    "provider plugin `example`: too many tokens".to_owned(),
                ),
                "provider plugin `example`: too many tokens",
            ),
            (
                provider_types::ProviderError::Unauthorized("invalid key".to_owned()),
                ProviderError::Unauthorized("provider plugin `example`: invalid key".to_owned()),
                "provider plugin `example`: invalid key",
            ),
            (
                provider_types::ProviderError::Unavailable("service is down".to_owned()),
                ProviderError::Unavailable("provider plugin `example`: service is down".to_owned()),
                "provider plugin `example`: service is down",
            ),
            (
                provider_types::ProviderError::Refused("request declined".to_owned()),
                ProviderError::Refused("provider plugin `example`: request declined".to_owned()),
                "provider plugin `example`: request declined",
            ),
            (
                provider_types::ProviderError::Other("invalid response".to_owned()),
                ProviderError::Other("provider plugin `example`: invalid response".to_owned()),
                "provider plugin `example`: invalid response",
            ),
        ];

        for (wire_error, expected, display) in errors {
            let error = map_provider_error(&provider, wire_error);
            assert_eq!(error.to_string(), display);
            assert_eq!(error, expected);
        }
    }

    #[test]
    fn provider_role_failures_preserve_the_source() {
        let error = ProviderError::RoleUnavailable {
            provider: "example".into(),
            source: lockgate::RoleError::WrongHost,
        };

        assert_eq!(
            error.to_string(),
            "provider plugin `example` failed: plugin handle belongs to a different Lockgate Host"
        );
        assert!(
            std::error::Error::source(&error)
                .and_then(|source| source.downcast_ref::<lockgate::RoleError>())
                .is_some_and(|source| matches!(source, lockgate::RoleError::WrongHost))
        );
    }

    #[test]
    fn provider_call_failures_preserve_the_source() {
        let error = map_plugin_call_error(
            &"example".into(),
            CallError::Trap {
                detail: "guest panicked".to_owned(),
            },
        );

        assert_eq!(
            error.to_string(),
            "provider plugin `example` failed: plugin trapped: guest panicked"
        );
        assert!(
            std::error::Error::source(&error)
                .and_then(|source| source.downcast_ref::<Arc<CallError>>())
                .is_some_and(|source| matches!(source.as_ref(), CallError::Trap { detail } if detail == "guest panicked"))
        );
    }

    #[test]
    fn provider_host_panics_name_the_import() {
        let error = map_plugin_call_error(
            &"example".into(),
            CallError::HostPanic {
                import: "chap:state/state.recall".to_owned(),
                message: "host invariant failed".to_owned(),
            },
        );

        assert_eq!(
            error.to_string(),
            "provider plugin `example` failed: host import `chap:state/state.recall` panicked: host invariant failed"
        );
        assert!(
            std::error::Error::source(&error)
                .and_then(|source| source.downcast_ref::<Arc<CallError>>())
                .is_some_and(|source| matches!(
                    source.as_ref(),
                    CallError::HostPanic { import, message }
                        if import == "chap:state/state.recall" && message == "host invariant failed"
                ))
        );
    }

    #[test]
    fn provider_deadlines_preserve_the_call_source() {
        let error = map_plugin_call_error(
            &"example".into(),
            CallError::DeadlineExceeded {
                deadline: Duration::from_secs(12),
            },
        );

        assert_eq!(
            error.to_string(),
            "provider plugin `example` timed out after 12s"
        );
        assert!(
            std::error::Error::source(&error)
                .and_then(|source| source.downcast_ref::<Arc<CallError>>())
                .is_some_and(|source| matches!(source.as_ref(), CallError::DeadlineExceeded { deadline } if *deadline == Duration::from_secs(12)))
        );
    }
}
