use chap_plugin::provider::{Completion, CompletionRequest, Provider, ProviderError};
use chap_plugin::{MetadataSource, Needs, Plugin, ScopeRef, env, net};
use schemars::JsonSchema;
use serde::Deserialize;

mod chat_completions;

struct OpenAiCompatible {
    settings: Settings,
}

chap_plugin::plugin!(OpenAiCompatible: Provider);

impl Plugin for OpenAiCompatible {
    const ID: &'static str = "openai";
    const DISPLAY_NAME: MetadataSource = MetadataSource::Explicit("OpenAI-compatible provider");
    const DESCRIPTION: MetadataSource =
        MetadataSource::Explicit("Calls an OpenAI-compatible Chat Completions server");
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::required(&[net::EGRESS.need(&[ScopeRef::setting("/base_url")])])
        .optional(&[env::READ.need(&[ScopeRef::setting("/api_key_env")])]);
    type Settings = Settings;

    fn new(settings: Self::Settings) -> Self {
        Self { settings }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Settings {
    #[schemars(regex(pattern = r"\S"))]
    base_url: String,
    #[schemars(regex(pattern = r"\S"))]
    model: String,
    api_key_env: Option<String>,
    #[serde(default = "replay_reasoning_by_default")]
    #[schemars(default = "replay_reasoning_by_default")]
    replay_reasoning: ReplayReasoning,
    /// Merged into every request body verbatim for server quirks.
    #[serde(default)]
    #[schemars(extend("propertyNames" = allowed_fragment_property_names()))]
    request_body: BodyFragment,
}

/// A raw JSON object merged into the request body. The keys belong to the
/// server, not to CHAP, and are sent unmodified.
#[derive(Clone, Default, Deserialize, JsonSchema)]
#[serde(transparent)]
struct BodyFragment(
    #[serde(deserialize_with = "deserialize_body_fragment")]
    serde_json::Map<String, serde_json::Value>,
);

const OWNED_REQUEST_FIELDS: [&str; 4] = ["model", "messages", "tools", "stream"];

fn allowed_fragment_property_names() -> serde_json::Value {
    serde_json::json!({ "not": { "enum": OWNED_REQUEST_FIELDS } })
}

fn deserialize_body_fragment<'de, D>(
    deserializer: D,
) -> Result<serde_json::Map<String, serde_json::Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let body: serde_json::Map<String, serde_json::Value> = Deserialize::deserialize(deserializer)?;
    if let Some(field) = OWNED_REQUEST_FIELDS
        .iter()
        .find(|field| body.contains_key(**field))
    {
        return Err(serde::de::Error::custom(format!(
            "request body fragments cannot set plugin-owned field `{field}`"
        )));
    }
    Ok(body)
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
enum ReplayReasoning {
    Mode(ReplayReasoningMode),
    Inline { inline: ReasoningDelimiters },
}

impl ReplayReasoning {
    fn parts(&self) -> (ReplayReasoningMode, Option<&ReasoningDelimiters>) {
        match self {
            Self::Mode(mode) => (*mode, None),
            Self::Inline { inline } => (ReplayReasoningMode::Inline, Some(inline)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
enum ReplayReasoningMode {
    Field,
    Inline,
    Off,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReasoningDelimiters {
    open: String,
    close: String,
}

const fn replay_reasoning_by_default() -> ReplayReasoning {
    // Dropping reasoning hands the model a transcript of its own turns that it never wrote.
    // Of the two ways to send it back, `reasoning_content` is the field a server supporting
    // replay reads, and one that does not ignores it. Inline replay has no such escape hatch:
    // it lands in `content`, where every server re-tokenizes it and the model imitates the
    // shape it finds there.
    ReplayReasoning::Mode(ReplayReasoningMode::Field)
}

impl Provider for OpenAiCompatible {
    async fn complete(&self, request: CompletionRequest) -> Result<Completion, ProviderError> {
        let settings = &self.settings;
        let request = chat_completions::encode_request(settings, request)?;
        let url = format!(
            "{}/chat/completions",
            settings.base_url.trim_end_matches('/')
        );
        let api_key = settings
            .api_key_env
            .as_deref()
            .map(read_environment_variable)
            .transpose()?;
        let response = chap_plugin::http::Client::new()
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

    fn settings_with(extra: serde_json::Value) -> Result<Settings, serde_json::Error> {
        let mut value = serde_json::json!({
            "base_url": "https://example.com/v1",
            "model": "example-model",
        });
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_value(value)
    }

    #[test]
    fn defaults_reasoning_replay_to_field() {
        assert_eq!(
            settings_with(serde_json::json!({}))
                .unwrap()
                .replay_reasoning,
            ReplayReasoning::Mode(ReplayReasoningMode::Field)
        );
    }

    #[test]
    fn parses_each_explicit_reasoning_replay_mode() {
        for (value, expected) in [
            ("field", ReplayReasoningMode::Field),
            ("inline", ReplayReasoningMode::Inline),
            ("off", ReplayReasoningMode::Off),
        ] {
            assert_eq!(
                settings_with(serde_json::json!({ "replay_reasoning": value }))
                    .unwrap()
                    .replay_reasoning,
                ReplayReasoning::Mode(expected)
            );
        }
    }

    #[test]
    fn rejects_an_unknown_reasoning_replay_mode() {
        assert!(settings_with(serde_json::json!({ "replay_reasoning": "unknown" })).is_err());
    }

    #[test]
    fn parses_inline_reasoning_delimiters() {
        let settings = settings_with(serde_json::json!({
            "replay_reasoning": {
                "inline": { "open": "<think>", "close": "</think>" },
            },
        }))
        .unwrap();

        assert_eq!(
            settings.replay_reasoning,
            ReplayReasoning::Inline {
                inline: ReasoningDelimiters {
                    open: "<think>".to_owned(),
                    close: "</think>".to_owned(),
                },
            }
        );
    }

    #[test]
    fn rejects_half_specified_reasoning_delimiters() {
        for reasoning_delimiters in [
            serde_json::json!({ "open": "[" }),
            serde_json::json!({ "close": "]" }),
        ] {
            assert!(
                settings_with(serde_json::json!({
                    "replay_reasoning": { "inline": reasoning_delimiters },
                }))
                .is_err()
            );
        }
    }

    #[test]
    fn defaults_to_an_empty_request_body() {
        let settings = settings_with(serde_json::json!({})).unwrap();

        assert!(settings.request_body.0.is_empty());
    }

    #[test]
    fn rejects_plugin_owned_fields_in_fragments() {
        let error = settings_with(serde_json::json!({
            "request_body": { "messages": [] },
        }))
        .err()
        .unwrap();

        assert!(error.to_string().contains("plugin-owned field"));
    }

    #[test]
    fn settings_schema_forbids_plugin_owned_fields_in_fragments() {
        let generator = schemars::generate::SchemaSettings::draft2020_12()
            .for_deserialize()
            .into_generator();
        let schema = serde_json::to_value(generator.into_root_schema_for::<Settings>()).unwrap();
        let expected = serde_json::json!({ "not": { "enum": OWNED_REQUEST_FIELDS } });

        assert_eq!(
            schema["properties"]["request_body"]["propertyNames"],
            expected
        );
    }

    #[test]
    fn settings_schema_constrains_reasoning_replay() {
        let generator = schemars::generate::SchemaSettings::draft2020_12()
            .for_deserialize()
            .into_generator();
        let schema = serde_json::to_value(generator.into_root_schema_for::<Settings>()).unwrap();

        assert_eq!(
            schema["$defs"]["ReplayReasoningMode"]["enum"],
            serde_json::json!(["field", "inline", "off"])
        );
        assert_eq!(
            schema["$defs"]["ReplayReasoning"]["anyOf"][1],
            serde_json::json!({
                "additionalProperties": false,
                "properties": {
                    "inline": { "$ref": "#/$defs/ReasoningDelimiters" },
                },
                "required": ["inline"],
                "type": "object",
            })
        );
        assert_eq!(
            schema["$defs"]["ReasoningDelimiters"]["required"],
            serde_json::json!(["open", "close"])
        );
    }
}
