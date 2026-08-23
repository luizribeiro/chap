use chap::provider::{Completion, CompletionRequest, ProviderError};
use chap_plugin as chap;
use chap_plugin::{MetadataSource, Needs, Plugin, Provider, ScopeRef, env, net};
use std::ops::Deref;

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

/// Settings whose cross-field rules hold. Nothing else in the crate constructs one, so a
/// value in hand is what lets `selected_effort` resolve without a fallback.
#[derive(chap::serde::Deserialize, chap::schemars::JsonSchema)]
#[serde(crate = "chap::serde", try_from = "SettingsInput")]
#[schemars(crate = "chap::schemars")]
struct Settings(SettingsInput);

impl Deref for Settings {
    type Target = SettingsInput;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(chap::serde::Deserialize, chap::schemars::JsonSchema)]
#[serde(crate = "chap::serde", rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(crate = "chap::schemars", rename_all = "kebab-case")]
struct SettingsInput {
    #[schemars(regex(pattern = r"\S"))]
    base_url: String,
    #[schemars(regex(pattern = r"\S"))]
    model: String,
    api_key_env: Option<String>,
    #[serde(default = "replay_reasoning_by_default")]
    #[schemars(default = "replay_reasoning_by_default")]
    replay_reasoning: ReplayReasoning,
    reasoning_delimiters: Option<ReasoningDelimiters>,
    /// Rungs this instance offers, least to most effort. Omit to expose no effort
    /// control at all.
    #[serde(default)]
    effort_levels: Vec<EffortLevel>,
    /// The rung applied to requests. Must name one of `effort-levels`.
    default_effort: Option<String>,
    /// Merged into every request body verbatim, for server quirks that are not
    /// per-rung.
    #[serde(default)]
    #[schemars(extend("propertyNames" = allowed_fragment_property_names()))]
    request_body: BodyFragment,
}

impl TryFrom<SettingsInput> for Settings {
    type Error = String;

    fn try_from(settings: SettingsInput) -> Result<Self, Self::Error> {
        if settings.reasoning_delimiters.is_some()
            && settings.replay_reasoning != ReplayReasoning::Inline
        {
            return Err("reasoning-delimiters requires replay-reasoning to be `inline`".to_owned());
        }
        for (index, level) in settings.effort_levels.iter().enumerate() {
            if level.name.trim().is_empty() {
                return Err("effort-level names must not be empty".to_owned());
            }
            if settings.effort_levels[..index]
                .iter()
                .any(|previous| previous.name == level.name)
            {
                return Err(format!(
                    "effort-level names must be unique; duplicate `{}`",
                    level.name
                ));
            }
        }
        if let Some(default_effort) = &settings.default_effort
            && !settings
                .effort_levels
                .iter()
                .any(|level| level.name == *default_effort)
        {
            return Err(format!(
                "default-effort `{default_effort}` does not name an effort-level"
            ));
        }

        Ok(Self(settings))
    }
}

impl Settings {
    fn selected_effort(&self) -> Option<&EffortLevel> {
        self.default_effort.as_deref().map(|name| {
            self.effort_levels
                .iter()
                .find(|level| level.name == name)
                .expect("settings validation ensures the default effort exists")
        })
    }
}

#[derive(Clone, chap::serde::Deserialize, chap::schemars::JsonSchema)]
#[serde(crate = "chap::serde", deny_unknown_fields)]
#[schemars(crate = "chap::schemars")]
struct EffortLevel {
    /// The name an operator and, later, a caller sees. CHAP never interprets it.
    #[schemars(regex(pattern = r"\S"))]
    name: String,
    #[allow(dead_code)]
    description: Option<String>,
    #[serde(default)]
    #[schemars(extend("propertyNames" = allowed_fragment_property_names()))]
    body: BodyFragment,
}

/// A raw JSON object merged into the request body. The keys belong to the
/// server, not to CHAP, and are sent unmodified.
#[derive(Clone, Default, chap::serde::Deserialize, chap::schemars::JsonSchema)]
#[serde(crate = "chap::serde", transparent)]
#[schemars(crate = "chap::schemars")]
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
    D: chap::serde::Deserializer<'de>,
{
    let body: serde_json::Map<String, serde_json::Value> =
        chap::serde::Deserialize::deserialize(deserializer)?;
    if let Some(field) = OWNED_REQUEST_FIELDS
        .iter()
        .find(|field| body.contains_key(**field))
    {
        return Err(chap::serde::de::Error::custom(format!(
            "request body fragments cannot set plugin-owned field `{field}`"
        )));
    }
    Ok(body)
}

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, chap::serde::Deserialize, chap::schemars::JsonSchema,
)]
#[serde(crate = "chap::serde", rename_all = "kebab-case")]
#[schemars(crate = "chap::schemars", rename_all = "kebab-case")]
enum ReplayReasoning {
    Field,
    Inline,
    Off,
}

#[derive(chap::serde::Deserialize, chap::schemars::JsonSchema)]
#[serde(crate = "chap::serde", deny_unknown_fields)]
#[schemars(crate = "chap::schemars")]
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
    ReplayReasoning::Field
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

    fn settings_with(extra: serde_json::Value) -> Result<Settings, serde_json::Error> {
        let mut value = serde_json::json!({
            "base-url": "https://example.com/v1",
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
            ReplayReasoning::Field
        );
    }

    #[test]
    fn parses_each_explicit_reasoning_replay_mode() {
        for (value, expected) in [
            ("field", ReplayReasoning::Field),
            ("inline", ReplayReasoning::Inline),
            ("off", ReplayReasoning::Off),
        ] {
            assert_eq!(
                settings_with(serde_json::json!({ "replay-reasoning": value }))
                    .unwrap()
                    .replay_reasoning,
                expected
            );
        }
    }

    #[test]
    fn rejects_an_unknown_reasoning_replay_mode() {
        assert!(settings_with(serde_json::json!({ "replay-reasoning": "unknown" })).is_err());
    }

    #[test]
    fn rejects_reasoning_delimiters_outside_inline_mode() {
        for replay_reasoning in ["field", "off"] {
            let error = settings_with(serde_json::json!({
                "replay-reasoning": replay_reasoning,
                "reasoning-delimiters": { "open": "[", "close": "]" },
            }))
            .err()
            .unwrap();

            assert!(error.to_string().contains("requires replay-reasoning"));
        }
    }

    #[test]
    fn rejects_half_specified_reasoning_delimiters() {
        for reasoning_delimiters in [
            serde_json::json!({ "open": "[" }),
            serde_json::json!({ "close": "]" }),
        ] {
            assert!(
                settings_with(serde_json::json!({
                    "replay-reasoning": "inline",
                    "reasoning-delimiters": reasoning_delimiters,
                }))
                .is_err()
            );
        }
    }

    #[test]
    fn accepts_no_effort_control() {
        let settings = settings_with(serde_json::json!({})).unwrap();

        assert!(settings.effort_levels.is_empty());
        assert!(settings.default_effort.is_none());
        assert!(settings.request_body.0.is_empty());
    }

    #[test]
    fn rejects_an_unknown_default_effort() {
        let error = settings_with(serde_json::json!({
            "effort-levels": [{ "name": "low" }],
            "default-effort": "high",
        }))
        .err()
        .unwrap();

        assert!(error.to_string().contains("does not name an effort-level"));
    }

    #[test]
    fn rejects_duplicate_or_empty_effort_level_names() {
        for (levels, expected) in [
            (
                serde_json::json!([{ "name": "low" }, { "name": "low" }]),
                "must be unique",
            ),
            (serde_json::json!([{ "name": "" }]), "must not be empty"),
        ] {
            let error = settings_with(serde_json::json!({ "effort-levels": levels }))
                .err()
                .unwrap();

            assert!(error.to_string().contains(expected));
        }
    }

    #[test]
    fn rejects_plugin_owned_fields_in_fragments() {
        for extra in [
            serde_json::json!({ "request-body": { "messages": [] } }),
            serde_json::json!({
                "effort-levels": [{ "name": "high", "body": { "model": "other" } }],
            }),
        ] {
            let error = settings_with(extra).err().unwrap();

            assert!(error.to_string().contains("plugin-owned field"));
        }
    }

    #[test]
    fn settings_schema_forbids_plugin_owned_fields_in_fragments() {
        let generator = chap::schemars::generate::SchemaSettings::draft2020_12()
            .for_deserialize()
            .into_generator();
        let schema = serde_json::to_value(generator.into_root_schema_for::<Settings>()).unwrap();
        let expected = serde_json::json!({ "not": { "enum": OWNED_REQUEST_FIELDS } });

        assert_eq!(
            schema["properties"]["request-body"]["propertyNames"],
            expected
        );
        assert_eq!(
            schema["$defs"]["EffortLevel"]["properties"]["body"]["propertyNames"],
            expected
        );
    }
}
