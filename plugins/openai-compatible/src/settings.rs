use chap_plugin::http::Client;
use core::{num::NonZeroU64, time::Duration};
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
    #[serde(default)]
    pub(crate) timeouts: Timeouts,
    #[serde(default = "replay_reasoning_by_default")]
    #[schemars(default = "replay_reasoning_by_default")]
    pub(crate) replay_reasoning: ReplayReasoning,
    /// Merged into every request body verbatim for server quirks.
    #[serde(default, deserialize_with = "deserialize_body_fragment")]
    #[schemars(extend("propertyNames" = allowed_fragment_property_names()))]
    pub(crate) request_body: serde_json::Map<String, serde_json::Value>,
}

#[derive(Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Timeouts {
    connect_seconds: Option<NonZeroU64>,
    first_byte_seconds: Option<NonZeroU64>,
    between_bytes_seconds: Option<NonZeroU64>,
}

type ConfigureTimeout = fn(Client, Duration) -> Client;

pub(crate) fn http_client(settings: &Settings) -> Client {
    let timeouts: [(Option<NonZeroU64>, ConfigureTimeout); 3] = [
        (
            settings.timeouts.connect_seconds,
            Client::with_connect_timeout,
        ),
        (
            settings.timeouts.first_byte_seconds,
            Client::with_first_byte_timeout,
        ),
        (
            settings.timeouts.between_bytes_seconds,
            Client::with_between_bytes_timeout,
        ),
    ];

    timeouts
        .into_iter()
        .fold(Client::new(), |client, (seconds, configure)| {
            seconds.map_or(client, |seconds| {
                configure(client, Duration::from_secs(seconds.get()))
            })
        })
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

    fn assert_client_timeouts(settings: &Settings, expected: [Option<u64>; 3]) {
        let client = format!("{:?}", http_client(settings));
        for (name, seconds) in [
            "connect_timeout",
            "first_byte_timeout",
            "between_bytes_timeout",
        ]
        .into_iter()
        .zip(expected)
        {
            let expected = format!("{name}: {:?}", seconds.map(Duration::from_secs));
            assert!(client.contains(&expected), "{client}");
        }
    }

    fn settings_schema() -> serde_json::Value {
        let generator = schemars::generate::SchemaSettings::draft2020_12()
            .for_deserialize()
            .into_generator();
        serde_json::to_value(generator.into_root_schema_for::<Settings>()).unwrap()
    }

    #[test]
    fn defaults_transport_timeouts_to_unset() {
        let settings = settings_with(serde_json::json!({})).unwrap();

        assert_client_timeouts(&settings, [None, None, None]);
    }

    #[test]
    fn parses_each_transport_timeout() {
        for (timeouts, expected) in [
            (
                serde_json::json!({ "connect_seconds": 11 }),
                [Some(11), None, None],
            ),
            (
                serde_json::json!({ "first_byte_seconds": 22 }),
                [None, Some(22), None],
            ),
            (
                serde_json::json!({ "between_bytes_seconds": 33 }),
                [None, None, Some(33)],
            ),
        ] {
            let settings = settings_with(serde_json::json!({ "timeouts": timeouts })).unwrap();

            assert_client_timeouts(&settings, expected);
        }
    }

    #[test]
    fn parses_all_transport_timeouts() {
        let settings = settings_with(serde_json::json!({
            "timeouts": {
                "connect_seconds": 10,
                "first_byte_seconds": 60,
                "between_bytes_seconds": 30,
            },
        }))
        .unwrap();

        assert_client_timeouts(&settings, [Some(10), Some(60), Some(30)]);
    }

    #[test]
    fn rejects_zero_transport_timeouts() {
        for timeouts in [
            serde_json::json!({ "connect_seconds": 0 }),
            serde_json::json!({ "first_byte_seconds": 0 }),
            serde_json::json!({ "between_bytes_seconds": 0 }),
        ] {
            assert!(settings_with(serde_json::json!({ "timeouts": timeouts })).is_err());
        }
    }

    #[test]
    fn rejects_unknown_transport_timeout() {
        assert!(
            settings_with(serde_json::json!({
                "timeouts": { "total_seconds": 90 },
            }))
            .is_err()
        );
    }

    #[test]
    fn settings_schema_constrains_transport_timeouts() {
        let schema = settings_schema();
        let timeouts = &schema["$defs"]["Timeouts"];

        assert_eq!(timeouts["additionalProperties"], false);
        assert!(timeouts.get("required").is_none());
        for field in [
            "connect_seconds",
            "first_byte_seconds",
            "between_bytes_seconds",
        ] {
            assert_eq!(timeouts["properties"][field]["minimum"], 1);
        }
        assert!(
            !schema["required"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("timeouts"))
        );
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
        let schema = settings_schema();
        let expected = serde_json::json!({ "not": { "enum": OWNED_REQUEST_FIELDS } });

        assert_eq!(
            schema["properties"]["request_body"]["propertyNames"],
            expected
        );
    }

    #[test]
    fn settings_schema_constrains_reasoning_replay() {
        let schema = settings_schema();

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
