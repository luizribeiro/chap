//! Wire format for the OpenAI Chat Completions API.

use chap::provider::{self, AssistantContent, Completion, CompletionRequest, FinishReason};
use chap_plugin as chap;
use serde::{Deserialize, Serialize};

pub(crate) fn encode_request(model: &str, request: CompletionRequest) -> Result<String, String> {
    serde_json::to_string(&Request {
        model,
        messages: request.messages.into_iter().map(Message::from).collect(),
        tools: request
            .tools
            .into_iter()
            .map(Tool::try_from)
            .collect::<Result<_, _>>()?,
        stream: false,
    })
    .map_err(|error| format!("failed to encode OpenAI-compatible request: {error}"))
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
        #[serde(skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        content: String,
        tool_call_id: String,
        name: String,
    },
}

impl From<provider::Message> for Message {
    fn from(message: provider::Message) -> Self {
        match message {
            provider::Message::System(content) => Self::System { content },
            provider::Message::User(content) => Self::User { content },
            provider::Message::Assistant(content) => {
                let mut text = Vec::new();
                let mut tool_calls = Vec::new();
                for content in content {
                    match content {
                        AssistantContent::Text(content) => text.push(content),
                        AssistantContent::ToolCall(call) => {
                            tool_calls.push(ToolCall::from(call));
                        }
                    }
                }
                Self::Assistant {
                    content: (!text.is_empty()).then(|| text.join("\n")),
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
    type Error = String;

    fn try_from(tool: provider::ToolDefinition) -> Result<Self, Self::Error> {
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

#[derive(Serialize)]
struct FunctionDefinition {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

pub(crate) fn parse_response(status: u16, body: &str) -> Result<Completion, String> {
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
        AssistantContent::ToolCall(provider::ToolCall {
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
    fn encodes_a_minimal_request() {
        let encoded = encode_request(
            "example-model",
            CompletionRequest {
                messages: vec![provider::Message::User("hello".to_owned())],
                tools: vec![],
            },
        )
        .unwrap();

        let encoded: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(encoded["model"], "example-model");
        assert_eq!(encoded["stream"], false);
        assert!(encoded.get("tools").is_none());
        assert_eq!(encoded["messages"][0]["role"], "user");
        assert_eq!(encoded["messages"][0]["content"], "hello");
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
        .map(Message::from)
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
    fn rejects_responses_without_choices() {
        let error = parse_response(200, r#"{"choices":[]}"#).unwrap_err();
        assert_eq!(
            error,
            "OpenAI-compatible server returned no completion choices"
        );
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
}
