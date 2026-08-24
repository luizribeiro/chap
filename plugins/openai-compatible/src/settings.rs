use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Settings {
    #[schemars(regex(pattern = r"\S"))]
    pub(crate) base_url: String,
    #[schemars(regex(pattern = r"\S"))]
    pub(crate) model: String,
    pub(crate) api_key_env: Option<String>,
    #[serde(default = "replay_reasoning_by_default")]
    #[schemars(default = "replay_reasoning_by_default")]
    pub(crate) replay_reasoning: ReplayReasoning,
    /// Merged into every request body verbatim for server quirks.
    #[serde(default, deserialize_with = "deserialize_body_fragment")]
    #[schemars(extend("propertyNames" = allowed_fragment_property_names()))]
    pub(crate) request_body: serde_json::Map<String, serde_json::Value>,
}

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
pub(crate) enum ReplayReasoning {
    Mode(ReplayReasoningMode),
    Inline { inline: ReasoningDelimiters },
}

impl ReplayReasoning {
    pub(crate) fn parts(&self) -> (ReplayReasoningMode, Option<&ReasoningDelimiters>) {
        match self {
            Self::Mode(mode) => (*mode, None),
            Self::Inline { inline } => (ReplayReasoningMode::Inline, Some(inline)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub(crate) enum ReplayReasoningMode {
    Field,
    Inline,
    Off,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReasoningDelimiters {
    pub(crate) open: String,
    pub(crate) close: String,
}

const fn replay_reasoning_by_default() -> ReplayReasoning {
    // Dropping reasoning hands the model a transcript of its own turns that it never wrote.
    // Of the two ways to send it back, `reasoning_content` is the field a server supporting
    // replay reads, and one that does not ignores it. Inline replay has no such escape hatch:
    // it lands in `content`, where every server re-tokenizes it and the model imitates the
    // shape it finds there.
    ReplayReasoning::Mode(ReplayReasoningMode::Field)
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

        assert!(settings.request_body.is_empty());
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
