//! Wire format for the OpenAI Chat Completions API.

use chap::provider::{
    self, AssistantContent, Completion, CompletionRequest, FinishReason, ProviderError, RateLimit,
    Usage,
};
use chap_plugin as chap;
use serde::{Deserialize, Serialize};

use crate::{ReasoningDelimiters, ReplayReasoning, Settings};

/// Fragments compose by adding or overriding top-level fields; they never remove or rename them.
pub(crate) fn encode_request(
    settings: &Settings,
    request: CompletionRequest,
) -> Result<String, ProviderError> {
    let request = Request {
        model: &settings.model,
        messages: request
            .messages
            .into_iter()
            .map(|message| {
                Message::from_provider(
                    message,
                    settings.replay_reasoning,
                    settings.reasoning_delimiters.as_ref(),
                )
            })
            .collect(),
        tools: request
            .tools
            .into_iter()
            .map(Tool::try_from)
            .collect::<Result<_, _>>()?,
        stream: false,
    };
    let selected = settings.selected_effort();
    if settings.request_body.0.is_empty() && selected.is_none_or(|level| level.body.0.is_empty()) {
        return serde_json::to_string(&request).map_err(encoding_error);
    }

    let mut body = serde_json::to_value(request).map_err(encoding_error)?;
    let object = body
        .as_object_mut()
        .expect("an OpenAI-compatible request encodes to an object");
    object.extend(settings.request_body.0.clone());
    if let Some(level) = selected {
        object.extend(level.body.0.clone());
    }
    serde_json::to_string(&body).map_err(encoding_error)
}

fn encoding_error(error: serde_json::Error) -> ProviderError {
    ProviderError::Other(format!(
        "failed to encode OpenAI-compatible request: {error}"
    ))
}

#[derive(Serialize)]
struct Request<'a> {
    model: &'a str,
    messages: Vec<Message>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Tool>,
    stream: bool,
}

#[derive(Serialize)]
#[serde(tag = "role")]
#[serde(rename_all = "lowercase")]
enum Message {
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning_content: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        content: String,
        tool_call_id: String,
        name: String,
    },
}

impl Message {
    fn from_provider(
        message: provider::Message,
        replay_reasoning: ReplayReasoning,
        reasoning_delimiters: Option<&ReasoningDelimiters>,
    ) -> Self {
        match message {
            provider::Message::System(content) => Self::System { content },
            provider::Message::User(content) => Self::User { content },
            provider::Message::Assistant(content) => {
                let mut text = Vec::new();
                let mut reasoning = Vec::new();
                let mut tool_calls = Vec::new();
                for content in content {
                    match content {
                        AssistantContent::Text(content) => text.push(content),
                        AssistantContent::Reasoning(content) => {
                            if !content.text.is_empty() {
                                reasoning.push(content.text);
                            }
                        }
                        AssistantContent::ToolCall(call) => {
                            tool_calls.push(ToolCall::from(call));
                        }
                    }
                }
                let text = (!text.is_empty()).then(|| text.join("\n"));
                let reasoning = (!reasoning.is_empty()).then(|| reasoning.join("\n"));
                let (content, reasoning_content) = match replay_reasoning {
                    ReplayReasoning::Field => (text, reasoning),
                    ReplayReasoning::Inline => {
                        // Untagged reasoning avoids teaching the model to mimic delimiters.
                        let reasoning = reasoning.map(|reasoning| match reasoning_delimiters {
                            Some(delimiters) => {
                                format!("{}{reasoning}{}", delimiters.open, delimiters.close)
                            }
                            None => reasoning,
                        });
                        (
                            match (reasoning, text) {
                                (Some(reasoning), Some(text)) => {
                                    Some(format!("{reasoning}\n\n{text}"))
                                }
                                (Some(reasoning), None) => Some(reasoning),
                                (None, text) => text,
                            },
                            None,
                        )
                    }
                    ReplayReasoning::Off => (text, None),
                };
                Self::Assistant {
                    content,
                    reasoning_content,
                    tool_calls,
                }
            }
            provider::Message::ToolResult(result) => Self::Tool {
                content: result.output,
                tool_call_id: result.call_id,
                name: result.name,
            },
        }
    }
}

#[derive(Serialize)]
struct ToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: &'static str,
    function: FunctionCall,
}

impl From<provider::ToolCall> for ToolCall {
    fn from(call: provider::ToolCall) -> Self {
        Self {
            id: call.id,
            kind: "function",
            function: FunctionCall {
                name: call.name,
                arguments: call.arguments,
            },
        }
    }
}

#[derive(Serialize)]
struct FunctionCall {
    name: String,
    arguments: String,
}

#[derive(Serialize)]
struct Tool {
    #[serde(rename = "type")]
    kind: &'static str,
    function: FunctionDefinition,
}

impl TryFrom<provider::ToolDefinition> for Tool {
    type Error = ProviderError;

    fn try_from(tool: provider::ToolDefinition) -> Result<Self, Self::Error> {
        Ok(Self {
            kind: "function",
            function: FunctionDefinition {
                name: tool.name,
                description: tool.description,
                parameters: serde_json::from_str(&tool.parameters).map_err(|error| {
                    ProviderError::Other(format!(
                        "tool parameters must be valid JSON Schema: {error}"
                    ))
                })?,
            },
        })
    }
}

#[derive(Serialize)]
struct FunctionDefinition {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

pub(crate) struct ResponseMetadata {
    pub(crate) status: u16,
    pub(crate) retry_after: Option<u64>,
}

pub(crate) fn parse_response(
    metadata: ResponseMetadata,
    body: &str,
) -> Result<Completion, ProviderError> {
    let ResponseMetadata {
        status,
        retry_after,
    } = metadata;
    if !(200..300).contains(&status) {
        let (provider_message, code) = serde_json::from_str::<ErrorResponse>(body)
            .map(|response| (response.error.message, response.error.code))
            .unwrap_or_else(|_| (body.chars().take(500).collect(), None));
        let context_too_long = looks_like_context_length(code.as_deref(), &provider_message);
        let message =
            format!("OpenAI-compatible server returned HTTP {status}: {provider_message}");
        return Err(match status {
            429 => ProviderError::RateLimited(RateLimit {
                retry_after,
                message,
            }),
            401 | 403 => ProviderError::Unauthorized(message),
            400 | 413 if context_too_long => ProviderError::ContextTooLong(message),
            500..=599 => ProviderError::Unavailable(message),
            _ => ProviderError::Other(message),
        });
    }
    let Response { choices, usage } = serde_json::from_str(body).map_err(|error| {
        ProviderError::Other(format!("invalid OpenAI-compatible response: {error}"))
    })?;
    let choice = choices.into_iter().next().ok_or_else(|| {
        ProviderError::Other("OpenAI-compatible server returned no completion choices".to_owned())
    })?;
    let Choice {
        message,
        finish_reason: completion_finish_reason,
    } = choice;
    let ResponseMessage {
        content: text,
        reasoning_content,
        reasoning,
        reasoning_text,
        tool_calls,
        refusal,
    } = message;
    let mut content = Vec::new();
    if let Some(text) = [reasoning_content, reasoning, reasoning_text]
        .into_iter()
        .flatten()
        .find(|text| !text.is_empty())
    {
        content.push(AssistantContent::Reasoning(provider::Reasoning {
            text,
            // Chat Completions has no replay-token concept.
            signature: None,
        }));
    }
    if let Some(text) = text {
        content.push(AssistantContent::Text(text));
    }
    content.extend(tool_calls.into_iter().map(|call| {
        AssistantContent::ToolCall(provider::ToolCall {
            id: call.id,
            name: call.function.name,
            arguments: call.function.arguments,
        })
    }));
    if content.is_empty() {
        return Err(match refusal {
            Some(refusal) => ProviderError::Refused(format!(
                "OpenAI-compatible server refused the request: {refusal}"
            )),
            None => ProviderError::Other(
                "OpenAI-compatible server returned a completion without content".to_owned(),
            ),
        });
    }
    Ok(Completion {
        content,
        finish_reason: finish_reason(completion_finish_reason),
        usage: usage.map(Usage::from),
    })
}

#[derive(Deserialize)]
struct Response {
    choices: Vec<Choice>,
    usage: Option<ResponseUsage>,
}

#[derive(Deserialize)]
struct ResponseUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    prompt_tokens_details: Option<PromptTokensDetails>,
    completion_tokens_details: Option<CompletionTokensDetails>,
}

#[derive(Deserialize)]
struct PromptTokensDetails {
    cached_tokens: Option<u64>,
    /// A vLLM extension, not part of the OpenAI schema:
    /// https://github.com/vllm-project/vllm/blob/d6c2fec9fd72eeac44f42c884e19a1c6bd1142a7/vllm/entrypoints/openai/engine/protocol.py#L110-L113
    created_cache_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct CompletionTokensDetails {
    reasoning_tokens: Option<u64>,
}

impl From<ResponseUsage> for Usage {
    fn from(usage: ResponseUsage) -> Self {
        Self {
            input_tokens: usage.prompt_tokens,
            cached_input_tokens: usage
                .prompt_tokens_details
                .as_ref()
                .and_then(|details| details.cached_tokens),
            cache_write_tokens: usage
                .prompt_tokens_details
                .and_then(|details| details.created_cache_tokens),
            output_tokens: usage.completion_tokens,
            reasoning_tokens: usage
                .completion_tokens_details
                .and_then(|details| details.reasoning_tokens),
        }
    }
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ResponseMessage {
    content: Option<String>,
    reasoning_content: Option<String>,
    reasoning: Option<String>,
    reasoning_text: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ResponseToolCall>,
    #[serde(default)]
    refusal: Option<String>,
}

#[derive(Deserialize)]
struct ResponseToolCall {
    id: String,
    function: ResponseFunctionCall,
}

#[derive(Deserialize)]
struct ResponseFunctionCall {
    name: String,
    arguments: String,
}

fn finish_reason(reason: Option<String>) -> FinishReason {
    match reason.as_deref() {
        None | Some("stop") => FinishReason::Stop,
        Some("tool_calls") => FinishReason::ToolCalls,
        Some("length") => FinishReason::Length,
        Some(reason) => FinishReason::Other(reason.to_owned()),
    }
}

#[derive(Deserialize)]
struct ErrorResponse {
    error: ErrorBody,
}

#[derive(Deserialize)]
struct ErrorBody {
    message: String,
    code: Option<String>,
}

fn looks_like_context_length(code: Option<&str>, message: &str) -> bool {
    if code == Some("context_length_exceeded") {
        return true;
    }

    // Misclassification changes host recovery, so keep the message fallback deliberately narrow.
    let message = message.to_ascii_lowercase();
    message.contains("context length") || message.contains("context window")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response_metadata(status: u16) -> ResponseMetadata {
        ResponseMetadata {
            status,
            retry_after: None,
        }
    }

    fn parse_api_error(
        metadata: ResponseMetadata,
        message: &str,
        code: Option<&str>,
    ) -> ProviderError {
        let body = serde_json::json!({
            "error": {
                "message": message,
                "code": code,
            }
        })
        .to_string();

        parse_response(metadata, &body).unwrap_err()
    }

    fn parse_completion_with_usage(usage: Option<&str>) -> Completion {
        let usage = usage
            .map(|usage| format!(r#", "usage": {usage}"#))
            .unwrap_or_default();
        let body = format!(
            r#"{{"choices":[{{"message":{{"role":"assistant","content":"hello"}},"finish_reason":"stop"}}]{usage}}}"#
        );

        parse_response(response_metadata(200), &body).unwrap()
    }

    fn parse_completion_message(message: serde_json::Value) -> Result<Completion, ProviderError> {
        let body = serde_json::json!({
            "choices": [{
                "message": message,
                "finish_reason": "stop",
            }],
        })
        .to_string();

        parse_response(response_metadata(200), &body)
    }

    fn encode_assistant(
        content: Vec<AssistantContent>,
        replay_reasoning: ReplayReasoning,
    ) -> serde_json::Value {
        encode_assistant_with_delimiters(content, replay_reasoning, None)
    }

    fn encode_assistant_with_delimiters(
        content: Vec<AssistantContent>,
        replay_reasoning: ReplayReasoning,
        reasoning_delimiters: Option<&ReasoningDelimiters>,
    ) -> serde_json::Value {
        serde_json::to_value(Message::from_provider(
            provider::Message::Assistant(content),
            replay_reasoning,
            reasoning_delimiters,
        ))
        .unwrap()
    }

    fn reasoning(text: &str) -> AssistantContent {
        AssistantContent::Reasoning(provider::Reasoning {
            text: text.to_owned(),
            signature: None,
        })
    }

    fn assistant_tool_call() -> AssistantContent {
        AssistantContent::ToolCall(provider::ToolCall {
            id: "call-1".to_owned(),
            name: "weather".to_owned(),
            arguments: r#"{"city":"Paris"}"#.to_owned(),
        })
    }

    fn settings(extra: serde_json::Value) -> Settings {
        let mut value = serde_json::json!({
            "base-url": "https://example.com/v1",
            "model": "example-model",
        });
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_value(value).unwrap()
    }

    fn encode_minimal_request(settings: &Settings) -> String {
        encode_request(
            settings,
            CompletionRequest {
                messages: vec![provider::Message::User("hello".to_owned())],
                tools: vec![],
            },
        )
        .unwrap()
    }

    #[test]
    fn encodes_an_unconfigured_request_identically() {
        let encoded = encode_minimal_request(&settings(serde_json::json!({})));

        assert_eq!(
            encoded,
            r#"{"model":"example-model","messages":[{"role":"user","content":"hello"}],"stream":false}"#
        );

        let encoded: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(encoded["model"], "example-model");
        assert_eq!(encoded["stream"], false);
        assert!(encoded.get("tools").is_none());
        assert_eq!(encoded["messages"][0]["role"], "user");
        assert_eq!(encoded["messages"][0]["content"], "hello");
    }

    #[test]
    fn merges_request_body_members() {
        let settings = settings(serde_json::json!({
            "request-body": { "temperature": 0.25, "server-option": true },
        }));
        let encoded: serde_json::Value =
            serde_json::from_str(&encode_minimal_request(&settings)).unwrap();

        assert_eq!(encoded["temperature"], 0.25);
        assert_eq!(encoded["server-option"], true);
    }

    #[test]
    fn merges_the_default_effort_body() {
        let settings = settings(serde_json::json!({
            "effort-levels": [
                { "name": "off", "body": { "server-effort": "none" } },
                { "name": "medium", "body": { "server-effort": "medium" } },
            ],
            "default-effort": "medium",
        }));
        let encoded: serde_json::Value =
            serde_json::from_str(&encode_minimal_request(&settings)).unwrap();

        assert_eq!(encoded["server-effort"], "medium");
    }

    #[test]
    fn default_effort_body_overrides_the_request_body() {
        let settings = settings(serde_json::json!({
            "request-body": { "temperature": 0.25 },
            "effort-levels": [{ "name": "high", "body": { "temperature": 0.75 } }],
            "default-effort": "high",
        }));
        let encoded: serde_json::Value =
            serde_json::from_str(&encode_minimal_request(&settings)).unwrap();

        assert_eq!(encoded["temperature"], 0.75);
    }

    #[test]
    fn preserves_nested_effort_body_objects() {
        let settings = settings(serde_json::json!({
            "effort-levels": [{
                "name": "off",
                "body": { "chat_template_kwargs": { "enable_thinking": false } },
            }],
            "default-effort": "off",
        }));
        let encoded: serde_json::Value =
            serde_json::from_str(&encode_minimal_request(&settings)).unwrap();

        assert_eq!(
            encoded["chat_template_kwargs"],
            serde_json::json!({ "enable_thinking": false })
        );
    }

    #[test]
    fn encodes_complete_conversation_history() {
        let messages = vec![
            provider::Message::System("be concise".to_owned()),
            provider::Message::User("hello".to_owned()),
            provider::Message::Assistant(vec![AssistantContent::Text("hi".to_owned())]),
            provider::Message::Assistant(vec![AssistantContent::ToolCall(provider::ToolCall {
                id: "call-1".to_owned(),
                name: "weather".to_owned(),
                arguments: r#"{"city":"Paris"}"#.to_owned(),
            })]),
            provider::Message::ToolResult(provider::ToolResult {
                call_id: "call-1".to_owned(),
                name: "weather".to_owned(),
                output: "sunny".to_owned(),
                is_error: false,
            }),
        ]
        .into_iter()
        .map(|message| Message::from_provider(message, ReplayReasoning::Field, None))
        .collect::<Vec<_>>();

        let encoded = serde_json::to_value(messages).unwrap();
        assert_eq!(encoded[0]["role"], "system");
        assert_eq!(encoded[1]["role"], "user");
        assert_eq!(encoded[2]["role"], "assistant");
        assert_eq!(encoded[3]["role"], "assistant");
        assert_eq!(encoded[3]["tool_calls"][0]["id"], "call-1");
        assert_eq!(encoded[4]["role"], "tool");
        assert_eq!(encoded[4]["tool_call_id"], "call-1");
        assert_eq!(encoded[4]["content"], "sunny");
    }

    #[test]
    fn field_encodes_reasoning_separately_from_content() {
        let encoded = encode_assistant(
            vec![
                reasoning("think"),
                AssistantContent::Text("answer".to_owned()),
            ],
            ReplayReasoning::Field,
        );

        assert_eq!(encoded["reasoning_content"], "think");
        assert_eq!(encoded["content"], "answer");
    }

    #[test]
    fn inline_encodes_untagged_reasoning_in_content_only() {
        let encoded = encode_assistant(
            vec![
                reasoning("think"),
                AssistantContent::Text("answer".to_owned()),
            ],
            ReplayReasoning::Inline,
        );

        assert_eq!(encoded["content"], "think\n\nanswer");
        assert!(!encoded.to_string().contains(['<', '>']));
        assert!(encoded.get("reasoning_content").is_none());
    }

    #[test]
    fn inline_wraps_only_reasoning_in_operator_delimiters() {
        let delimiters = ReasoningDelimiters {
            open: "<think>".to_owned(),
            close: "</think>".to_owned(),
        };
        let encoded = encode_assistant_with_delimiters(
            vec![
                reasoning("think"),
                AssistantContent::Text("answer".to_owned()),
            ],
            ReplayReasoning::Inline,
            Some(&delimiters),
        );

        assert_eq!(encoded["content"], "<think>think</think>\n\nanswer");
        assert!(encoded.get("reasoning_content").is_none());
    }

    #[test]
    fn off_omits_reasoning_from_the_message() {
        let encoded = encode_assistant(vec![reasoning("think")], ReplayReasoning::Off);

        assert!(encoded.get("reasoning_content").is_none());
        assert!(encoded.get("content").is_none());
    }

    #[test]
    fn joins_multiple_reasoning_parts_inline() {
        let encoded = encode_assistant(
            vec![reasoning("first"), reasoning("second")],
            ReplayReasoning::Inline,
        );

        assert_eq!(encoded["content"], "first\nsecond");
        assert!(encoded.get("reasoning_content").is_none());
    }

    #[test]
    fn inline_preserves_reasoning_without_text() {
        let encoded = encode_assistant(vec![reasoning("think")], ReplayReasoning::Inline);

        assert_eq!(encoded["content"], "think");
        assert!(encoded.get("reasoning_content").is_none());
    }

    #[test]
    fn preserves_text_and_tool_calls_in_every_reasoning_replay_mode() {
        for (replay_reasoning, expected_content) in [
            (ReplayReasoning::Field, "hello"),
            (ReplayReasoning::Inline, "think\n\nhello"),
            (ReplayReasoning::Off, "hello"),
        ] {
            let encoded = encode_assistant(
                vec![
                    reasoning("think"),
                    AssistantContent::Text("hello".to_owned()),
                    assistant_tool_call(),
                ],
                replay_reasoning,
            );

            assert_eq!(encoded["content"], expected_content);
            assert_eq!(encoded["tool_calls"][0]["id"], "call-1");
        }
    }

    #[test]
    fn omits_empty_reasoning_in_every_replay_mode() {
        for replay_reasoning in [
            ReplayReasoning::Field,
            ReplayReasoning::Inline,
            ReplayReasoning::Off,
        ] {
            let encoded = encode_assistant(vec![reasoning("")], replay_reasoning);

            assert!(encoded.get("reasoning_content").is_none());
            assert!(encoded.get("content").is_none());
        }
    }

    #[test]
    fn encodes_tool_definitions() {
        let tool = Tool::try_from(provider::ToolDefinition {
            name: "weather".to_owned(),
            description: "Get the weather".to_owned(),
            parameters: r#"{"type":"object","properties":{"city":{"type":"string"}}}"#.to_owned(),
        })
        .unwrap();

        let encoded = serde_json::to_value(tool).unwrap();
        assert_eq!(encoded["type"], "function");
        assert_eq!(encoded["function"]["name"], "weather");
        assert_eq!(
            encoded["function"]["parameters"]["properties"]["city"]["type"],
            "string"
        );
    }

    #[test]
    fn extracts_the_first_completion() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}]}"#;
        let completion = parse_response(response_metadata(200), body).unwrap();

        assert!(matches!(
            completion.content.as_slice(),
            [AssistantContent::Text(text)] if text == "hello"
        ));
        assert!(matches!(completion.finish_reason, FinishReason::Stop));
    }

    #[test]
    fn extracts_each_supported_reasoning_field() {
        for message in [
            serde_json::json!({"reasoning_content": "think"}),
            serde_json::json!({"reasoning": "think"}),
            serde_json::json!({"reasoning_text": "think"}),
        ] {
            let completion = parse_completion_message(message).unwrap();

            assert!(matches!(
                completion.content.as_slice(),
                [AssistantContent::Reasoning(reasoning)]
                    if reasoning.text == "think" && reasoning.signature.is_none()
            ));
        }
    }

    #[test]
    fn takes_the_first_nonempty_reasoning_field() {
        let completion = parse_completion_message(serde_json::json!({
            "reasoning_content": "first",
            "reasoning": "second",
            "reasoning_text": "third",
        }))
        .unwrap();

        assert!(matches!(
            completion.content.as_slice(),
            [AssistantContent::Reasoning(reasoning)] if reasoning.text == "first"
        ));
    }

    #[test]
    fn treats_empty_reasoning_as_absent() {
        let completion = parse_completion_message(serde_json::json!({
            "content": "hello",
            "reasoning_content": "",
        }))
        .unwrap();

        assert!(matches!(
            completion.content.as_slice(),
            [AssistantContent::Text(text)] if text == "hello"
        ));
    }

    #[test]
    fn places_reasoning_before_response_text() {
        let completion = parse_completion_message(serde_json::json!({
            "content": "answer",
            "reasoning_content": "think",
        }))
        .unwrap();

        assert!(matches!(
            completion.content.as_slice(),
            [
                AssistantContent::Reasoning(reasoning),
                AssistantContent::Text(text),
            ] if reasoning.text == "think" && text == "answer"
        ));
    }

    #[test]
    fn accepts_a_reasoning_only_response() {
        let completion = parse_completion_message(serde_json::json!({
            "content": null,
            "reasoning_content": "think",
        }))
        .unwrap();

        assert!(matches!(
            completion.content.as_slice(),
            [AssistantContent::Reasoning(reasoning)] if reasoning.text == "think"
        ));
    }

    #[test]
    fn rejects_a_response_without_reasoning_text_or_tool_calls() {
        let error = parse_completion_message(serde_json::json!({"content": null})).unwrap_err();

        assert!(matches!(
            error,
            ProviderError::Other(message)
                if message == "OpenAI-compatible server returned a completion without content"
        ));
    }

    #[test]
    fn extracts_usage() {
        let completion = parse_completion_with_usage(Some(
            r#"{"prompt_tokens":53,"completion_tokens":58,"prompt_tokens_details":{"cached_tokens":20,"created_cache_tokens":17},"completion_tokens_details":{"reasoning_tokens":45},"total_tokens":111}"#,
        ));

        assert!(matches!(
            completion.usage,
            Some(Usage {
                input_tokens: 53,
                cached_input_tokens: Some(20),
                cache_write_tokens: Some(17),
                output_tokens: 58,
                reasoning_tokens: Some(45),
            })
        ));
    }

    #[test]
    fn leaves_absent_usage_details_unreported() {
        let completion =
            parse_completion_with_usage(Some(r#"{"prompt_tokens":12,"completion_tokens":34}"#));

        assert!(matches!(
            completion.usage,
            Some(Usage {
                input_tokens: 12,
                cached_input_tokens: None,
                cache_write_tokens: None,
                output_tokens: 34,
                reasoning_tokens: None,
            })
        ));
    }

    #[test]
    fn leaves_null_usage_details_unreported() {
        let completion = parse_completion_with_usage(Some(
            r#"{"prompt_tokens":12,"completion_tokens":34,"prompt_tokens_details":null,"completion_tokens_details":null}"#,
        ));

        assert!(matches!(
            completion.usage,
            Some(Usage {
                input_tokens: 12,
                cached_input_tokens: None,
                cache_write_tokens: None,
                output_tokens: 34,
                reasoning_tokens: None,
            })
        ));
    }

    #[test]
    fn reports_no_usage_when_omitted() {
        let completion = parse_completion_with_usage(None);

        assert!(completion.usage.is_none());
    }

    #[test]
    fn reports_no_usage_when_null() {
        let completion = parse_completion_with_usage(Some("null"));

        assert!(completion.usage.is_none());
    }

    #[test]
    fn leaves_missing_usage_counters_unreported() {
        let completion = parse_completion_with_usage(Some(
            r#"{"prompt_tokens":12,"prompt_tokens_details":{},"completion_tokens_details":{}}"#,
        ));

        assert!(matches!(
            completion.usage,
            Some(Usage {
                input_tokens: 12,
                cached_input_tokens: None,
                cache_write_tokens: None,
                output_tokens: 0,
                reasoning_tokens: None,
            })
        ));
    }

    #[test]
    fn ignores_unknown_usage_fields() {
        let completion = parse_completion_with_usage(Some(
            r#"{"prompt_tokens":12,"completion_tokens":34,"service_tier":"default"}"#,
        ));

        assert!(matches!(
            completion.usage,
            Some(Usage {
                input_tokens: 12,
                cached_input_tokens: None,
                cache_write_tokens: None,
                output_tokens: 34,
                reasoning_tokens: None,
            })
        ));
    }

    #[test]
    fn extracts_tool_calls() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call-1","type":"function","function":{"name":"weather","arguments":"{\"city\":\"Paris\"}"}}]},"finish_reason":"tool_calls"}]}"#;
        let completion = parse_response(response_metadata(200), body).unwrap();

        assert!(matches!(
            completion.content.as_slice(),
            [AssistantContent::ToolCall(call)]
                if call.id == "call-1"
                    && call.name == "weather"
                    && call.arguments == r#"{"city":"Paris"}"#
        ));
        assert!(matches!(completion.finish_reason, FinishReason::ToolCalls));
    }

    #[test]
    fn rejects_responses_without_choices() {
        let error = parse_response(response_metadata(200), r#"{"choices":[]}"#).unwrap_err();
        assert!(matches!(
            error,
            ProviderError::Other(message)
                if message == "OpenAI-compatible server returned no completion choices"
        ));
    }

    #[test]
    fn classifies_rate_limits_with_retry_after() {
        for (header, expected) in [
            (Some("30"), Some(30)),
            (Some("Wed, 21 Oct 2015 07:28:00 GMT"), None),
            (None, None),
        ] {
            let error = parse_api_error(
                ResponseMetadata {
                    status: 429,
                    retry_after: crate::parse_retry_after(header),
                },
                "slow down",
                None,
            );
            assert!(matches!(
                error,
                ProviderError::RateLimited(RateLimit {
                    retry_after,
                    message,
                }) if retry_after == expected
                    && message == "OpenAI-compatible server returned HTTP 429: slow down"
            ));
        }
    }

    #[test]
    fn classifies_authentication_and_authorization_errors() {
        for status in [401, 403] {
            let error = parse_api_error(response_metadata(status), "invalid key", None);
            assert!(matches!(
                error,
                ProviderError::Unauthorized(message)
                    if message
                        == format!(
                            "OpenAI-compatible server returned HTTP {status}: invalid key"
                        )
            ));
        }
    }

    #[test]
    fn classifies_structured_context_length_errors() {
        for status in [400, 413] {
            let error = parse_api_error(
                response_metadata(status),
                "request is too large",
                Some("context_length_exceeded"),
            );
            assert!(matches!(error, ProviderError::ContextTooLong(_)));
        }
    }

    #[test]
    fn classifies_context_window_messages_without_a_code() {
        let error = parse_api_error(
            response_metadata(400),
            "This model's CONTEXT WINDOW is too small",
            None,
        );
        assert!(matches!(error, ProviderError::ContextTooLong(_)));
    }

    #[test]
    fn leaves_ordinary_bad_requests_unclassified() {
        let error = parse_api_error(response_metadata(400), "unknown model", None);
        assert!(matches!(error, ProviderError::Other(_)));
    }

    #[test]
    fn classifies_server_errors_as_unavailable() {
        let error = parse_api_error(response_metadata(503), "try later", None);
        assert!(matches!(error, ProviderError::Unavailable(_)));
    }

    #[test]
    fn leaves_other_statuses_unclassified() {
        let error = parse_api_error(response_metadata(404), "unknown model", None);
        assert!(matches!(
            error,
            ProviderError::Other(message)
                if message == "OpenAI-compatible server returned HTTP 404: unknown model"
        ));
    }

    #[test]
    fn classifies_refusals() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":null,"refusal":"I cannot help with that"},"finish_reason":"stop"}]}"#;
        let error = parse_response(response_metadata(200), body).unwrap_err();
        assert!(matches!(
            error,
            ProviderError::Refused(message)
                if message
                    == "OpenAI-compatible server refused the request: I cannot help with that"
        ));
    }
}
