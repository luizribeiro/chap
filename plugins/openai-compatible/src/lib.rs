mod bindings {
    lockgate_plugin::bindings!({
        path: "../../wit",
        world: "provider-plugin",
        metadata: {
            id: "openai",
            name: "OpenAI-compatible provider",
            version: "0.1.0",
            description: "Calls an OpenAI-compatible Chat Completions server",
        },
    });
}

use bindings::exports::sage::agent::provider::{
    AssistantContent, Completion, CompletionRequest, FinishReason, Guest,
    Message as ProviderMessage, ToolCall as ProviderToolCall, ToolDefinition as ProviderTool,
};
use bindings::sage::agent::settings;
use http::{HeaderMap, HeaderName, HeaderValue};
use http_body_util::BodyExt;
use serde::{Deserialize, Serialize};
use wasi_fetch::Client;

const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

struct OpenAiCompatible;

impl Guest for OpenAiCompatible {
    async fn complete(request: CompletionRequest) -> Result<Completion, String> {
        let base_url = required_setting("base-url").await?;
        let model = required_setting("model").await?;
        let request = serde_json::to_string(&Request {
            model,
            messages: request
                .messages
                .into_iter()
                .map(ChatCompletionMessage::from)
                .collect(),
            tools: request
                .tools
                .into_iter()
                .map(ChatCompletionTool::try_from)
                .collect::<Result<_, _>>()?,
            stream: false,
        })
        .map_err(|error| format!("failed to encode OpenAI-compatible request: {error}"))?;
        let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
        let mut headers = vec![("content-type".to_owned(), "application/json".to_owned())];
        if let Some(api_key) = settings::get("api-key".to_owned())
            .await
            .filter(|key| !key.is_empty())
        {
            headers.push(("authorization".to_owned(), format!("Bearer {api_key}")));
        }
        let (status, body) = post(&url, &headers, request.as_bytes()).await?;
        parse_response(status, &body)
    }
}

async fn required_setting(key: &str) -> Result<String, String> {
    settings::get(key.to_owned())
        .await
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("provider setting `{key}` is required"))
}

async fn post(
    url: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<(u16, String), String> {
    let mut request_headers = HeaderMap::new();
    for (name, value) in headers {
        let name = HeaderName::try_from(name)
            .map_err(|error| format!("invalid HTTP header name: {error}"))?;
        let value = HeaderValue::try_from(value)
            .map_err(|error| format!("invalid HTTP header value: {error}"))?;
        request_headers.append(name, value);
    }

    let response = Client::new()
        .post(url)
        .headers(request_headers)
        .body(body.to_vec())
        .send()
        .await
        .map_err(|error| format!("HTTP request failed: {error}"))?;
    let status = response.status().as_u16();
    let body = collect_limited(response.into_body(), MAX_RESPONSE_BYTES).await?;
    let body = String::from_utf8(body)
        .map_err(|error| format!("HTTP response body was not valid UTF-8: {error}"))?;
    Ok((status, body))
}

async fn collect_limited(mut body: wasi_fetch::Body, limit: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|error| format!("failed to read HTTP response body: {error}"))?;
        let Ok(chunk) = frame.into_data() else {
            continue;
        };
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(format!("HTTP response exceeded the {limit}-byte limit"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn parse_response(status: u16, body: &str) -> Result<Completion, String> {
    if !(200..300).contains(&status) {
        let message = serde_json::from_str::<ErrorResponse>(body)
            .ok()
            .map(|response| response.error.message)
            .unwrap_or_else(|| body.chars().take(500).collect());
        return Err(format!(
            "OpenAI-compatible server returned HTTP {status}: {message}"
        ));
    }
    let response: Response = serde_json::from_str(body)
        .map_err(|error| format!("invalid OpenAI-compatible response: {error}"))?;
    let choice = response
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| "OpenAI-compatible server returned no completion choices".to_owned())?;
    let mut content = Vec::new();
    if let Some(text) = choice.message.content {
        content.push(AssistantContent::Text(text));
    }
    content.extend(choice.message.tool_calls.into_iter().map(|call| {
        AssistantContent::ToolCall(ProviderToolCall {
            id: call.id,
            name: call.function.name,
            arguments: call.function.arguments,
        })
    }));
    if content.is_empty() {
        return Err(match choice.message.refusal {
            Some(refusal) => format!("OpenAI-compatible server refused the request: {refusal}"),
            None => "OpenAI-compatible server returned a completion without content".to_owned(),
        });
    }
    Ok(Completion {
        content,
        finish_reason: finish_reason(choice.finish_reason),
    })
}

#[derive(Serialize)]
struct Request {
    model: String,
    messages: Vec<ChatCompletionMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ChatCompletionTool>,
    stream: bool,
}

#[derive(Serialize)]
#[serde(tag = "role")]
#[serde(rename_all = "lowercase")]
enum ChatCompletionMessage {
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ChatCompletionToolCall>,
    },
    Tool {
        content: String,
        tool_call_id: String,
        name: String,
    },
}

impl From<ProviderMessage> for ChatCompletionMessage {
    fn from(message: ProviderMessage) -> Self {
        match message {
            ProviderMessage::System(content) => Self::System { content },
            ProviderMessage::User(content) => Self::User { content },
            ProviderMessage::Assistant(content) => {
                let mut text = Vec::new();
                let mut tool_calls = Vec::new();
                for content in content {
                    match content {
                        AssistantContent::Text(content) => text.push(content),
                        AssistantContent::ToolCall(call) => {
                            tool_calls.push(ChatCompletionToolCall::from(call));
                        }
                    }
                }
                Self::Assistant {
                    content: (!text.is_empty()).then(|| text.join("\n")),
                    tool_calls,
                }
            }
            ProviderMessage::ToolResult(result) => Self::Tool {
                content: result.output,
                tool_call_id: result.call_id,
                name: result.name,
            },
        }
    }
}

#[derive(Serialize)]
struct ChatCompletionToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: &'static str,
    function: FunctionCall,
}

impl From<ProviderToolCall> for ChatCompletionToolCall {
    fn from(call: ProviderToolCall) -> Self {
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
struct ChatCompletionTool {
    #[serde(rename = "type")]
    kind: &'static str,
    function: FunctionDefinition,
}

#[derive(Serialize)]
struct FunctionDefinition {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

impl TryFrom<ProviderTool> for ChatCompletionTool {
    type Error = String;

    fn try_from(tool: ProviderTool) -> Result<Self, Self::Error> {
        Ok(Self {
            kind: "function",
            function: FunctionDefinition {
                name: tool.name,
                description: tool.description,
                parameters: serde_json::from_str(&tool.parameters).map_err(|error| {
                    format!("tool parameters must be valid JSON Schema: {error}")
                })?,
            },
        })
    }
}

#[derive(Deserialize)]
struct Response {
    choices: Vec<Choice>,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_the_first_completion() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}]}"#;
        let completion = parse_response(200, body).unwrap();

        assert!(matches!(
            completion.content.as_slice(),
            [AssistantContent::Text(text)] if text == "hello"
        ));
        assert!(matches!(completion.finish_reason, FinishReason::Stop));
    }

    #[test]
    fn extracts_tool_calls() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call-1","type":"function","function":{"name":"weather","arguments":"{\"city\":\"Paris\"}"}}]},"finish_reason":"tool_calls"}]}"#;
        let completion = parse_response(200, body).unwrap();

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
    fn reports_structured_api_errors() {
        let body = r#"{"error":{"message":"unknown model"}}"#;
        let error = parse_response(404, body).unwrap_err();
        assert_eq!(
            error,
            "OpenAI-compatible server returned HTTP 404: unknown model"
        );
    }

    #[test]
    fn rejects_responses_without_choices() {
        let error = parse_response(200, r#"{"choices":[]}"#).unwrap_err();
        assert_eq!(
            error,
            "OpenAI-compatible server returned no completion choices"
        );
    }

    #[test]
    fn encodes_complete_conversation_history() {
        let messages = vec![
            ProviderMessage::System("be concise".to_owned()),
            ProviderMessage::User("hello".to_owned()),
            ProviderMessage::Assistant(vec![AssistantContent::Text("hi".to_owned())]),
            ProviderMessage::Assistant(vec![AssistantContent::ToolCall(ProviderToolCall {
                id: "call-1".to_owned(),
                name: "weather".to_owned(),
                arguments: r#"{"city":"Paris"}"#.to_owned(),
            })]),
            ProviderMessage::ToolResult(bindings::exports::sage::agent::provider::ToolResult {
                call_id: "call-1".to_owned(),
                name: "weather".to_owned(),
                output: "sunny".to_owned(),
                is_error: false,
            }),
        ]
        .into_iter()
        .map(ChatCompletionMessage::from)
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
    fn encodes_tool_definitions() {
        let tool = ChatCompletionTool::try_from(ProviderTool {
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
}

bindings::export!(OpenAiCompatible with_types_in bindings);
