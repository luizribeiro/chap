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

use bindings::exports::sage::agent::provider::Guest;
use bindings::sage::agent::{http_client, settings};
use serde::{Deserialize, Serialize};

struct OpenAiCompatible;

impl Guest for OpenAiCompatible {
    fn complete(prompt: String) -> Result<String, String> {
        let base_url = required_setting("base-url")?;
        let model = required_setting("model")?;
        let request = serde_json::to_string(&Request {
            model,
            messages: vec![Message {
                role: "user",
                content: prompt,
            }],
            stream: false,
        })
        .map_err(|error| format!("failed to encode OpenAI-compatible request: {error}"))?;
        let mut headers = vec![http_client::Header {
            name: "content-type".to_owned(),
            value: "application/json".to_owned(),
        }];
        if let Some(api_key) = settings::get("api-key").filter(|key| !key.is_empty()) {
            headers.push(http_client::Header {
                name: "authorization".to_owned(),
                value: format!("Bearer {api_key}"),
            });
        }
        let response = http_client::post(
            &format!("{}/chat/completions", base_url.trim_end_matches('/')),
            &headers,
            &request,
        )?;
        parse_response(response.status, &response.body)
    }
}

fn required_setting(key: &str) -> Result<String, String> {
    settings::get(key)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("provider setting `{key}` is required"))
}

fn parse_response(status: u16, body: &str) -> Result<String, String> {
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
    let message = response
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| "OpenAI-compatible server returned no completion choices".to_owned())?
        .message;
    message.content.ok_or_else(|| match message.refusal {
        Some(refusal) => format!("OpenAI-compatible server refused the request: {refusal}"),
        None => "OpenAI-compatible server returned a completion without text".to_owned(),
    })
}

#[derive(Serialize)]
struct Request {
    model: String,
    messages: Vec<Message>,
    stream: bool,
}

#[derive(Serialize)]
struct Message {
    role: &'static str,
    content: String,
}

#[derive(Deserialize)]
struct Response {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Deserialize)]
struct ResponseMessage {
    content: Option<String>,
    #[serde(default)]
    refusal: Option<String>,
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
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"hello"}}]}"#;
        assert_eq!(parse_response(200, body).unwrap(), "hello");
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
}

bindings::export!(OpenAiCompatible with_types_in bindings);
