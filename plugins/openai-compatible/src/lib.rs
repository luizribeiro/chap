use chap::provider::{Completion, CompletionRequest, ProviderError};
use chap_plugin as chap;
use chap_plugin::{MetadataSource, Needs, Plugin, Provider, ScopeRef, env, net};

mod chat_completions;

struct OpenAiCompatible {
    settings: Settings,
}

chap::plugin!(OpenAiCompatible: Provider);

impl Plugin for OpenAiCompatible {
    const ID: &'static str = "openai";
    const DISPLAY_NAME: MetadataSource = MetadataSource::Explicit("OpenAI-compatible provider");
    const DESCRIPTION: MetadataSource =
        MetadataSource::Explicit("Calls an OpenAI-compatible Chat Completions server");
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::required(&[net::EGRESS.need(&[ScopeRef::setting("/base-url")])])
        .optional(&[env::READ.need(&[ScopeRef::setting("/api-key-env")])]);
    type Settings = Settings;

    fn new(settings: Self::Settings) -> Self {
        Self { settings }
    }
}

#[derive(chap::serde::Deserialize, chap::schemars::JsonSchema)]
#[serde(crate = "chap::serde", rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(crate = "chap::schemars", rename_all = "kebab-case")]
struct Settings {
    #[schemars(regex(pattern = r"\S"))]
    base_url: String,
    #[schemars(regex(pattern = r"\S"))]
    model: String,
    api_key_env: Option<String>,
    #[serde(default = "replay_reasoning_by_default")]
    #[schemars(default = "replay_reasoning_by_default")]
    replay_reasoning: ReplayReasoning,
}

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, chap::serde::Deserialize, chap::schemars::JsonSchema,
)]
#[serde(crate = "chap::serde", rename_all = "kebab-case")]
#[schemars(crate = "chap::schemars", rename_all = "kebab-case")]
enum ReplayReasoning {
    Field,
    ThinkTags,
    Off,
}

const fn replay_reasoning_by_default() -> ReplayReasoning {
    // Replaying reasoning_content alongside tool definitions makes at least one Qwen-family
    // server emit multiple replies per turn. Operators can opt in where their server handles it.
    ReplayReasoning::Off
}

impl Provider for OpenAiCompatible {
    async fn complete(&self, request: CompletionRequest) -> Result<Completion, ProviderError> {
        let settings = &self.settings;
        let request =
            chat_completions::encode_request(&settings.model, settings.replay_reasoning, request)?;
        let url = format!(
            "{}/chat/completions",
            settings.base_url.trim_end_matches('/')
        );
        let api_key = settings
            .api_key_env
            .as_deref()
            .map(read_environment_variable)
            .transpose()?;
        let response = chap::http::Client::new()
            .post(&url)
            .header("content-type", "application/json")
            .map_err(|error| ProviderError::Other(error.to_string()))?
            .bearer(api_key.as_deref().filter(|api_key| !api_key.is_empty()))
            .body(request.into_bytes())
            .send()
            .await
            .map_err(|error| ProviderError::Unavailable(error.to_string()))?;
        let metadata = chat_completions::ResponseMetadata {
            status: response.status(),
            retry_after: parse_retry_after(
                response
                    .headers()
                    .get("retry-after")
                    .and_then(|value| value.to_str().ok()),
            ),
        };
        let body = response
            .text()
            .map_err(|error| ProviderError::Other(error.to_string()))?;
        chat_completions::parse_response(metadata, &body)
    }
}

fn read_environment_variable(name: &str) -> Result<String, ProviderError> {
    std::env::var(name).map_err(|_| {
        ProviderError::Unauthorized(format!(
            "environment variable `{name}` is not available to the plugin"
        ))
    })
}

fn parse_retry_after(value: Option<&str>) -> Option<u64> {
    // Retry-After also permits an HTTP-date; omit that form rather than add date parsing here.
    value?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(replay_reasoning: Option<&str>) -> Result<Settings, serde_json::Error> {
        let mut settings = serde_json::json!({
            "base-url": "https://example.com/v1",
            "model": "example-model",
        });
        if let Some(replay_reasoning) = replay_reasoning {
            settings["replay-reasoning"] = replay_reasoning.into();
        }
        serde_json::from_value(settings)
    }

    #[test]
    fn defaults_reasoning_replay_to_off() {
        assert_eq!(
            settings(None).unwrap().replay_reasoning,
            ReplayReasoning::Off
        );
    }

    #[test]
    fn parses_each_explicit_reasoning_replay_mode() {
        for (value, expected) in [
            ("field", ReplayReasoning::Field),
            ("think-tags", ReplayReasoning::ThinkTags),
            ("off", ReplayReasoning::Off),
        ] {
            assert_eq!(settings(Some(value)).unwrap().replay_reasoning, expected);
        }
    }

    #[test]
    fn rejects_an_unknown_reasoning_replay_mode() {
        assert!(settings(Some("unknown")).is_err());
    }
}
