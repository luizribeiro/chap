use chap_plugin::tools::{ToolDefinition, ToolError, Tools};
use chap_plugin::{MetadataSource, Needs, Plugin, capabilities};
use schemars::JsonSchema;
use serde::Deserialize;

const REMEMBER: &str = "remember";
const RECALL: &str = "recall";

struct Memory;

impl Plugin for Memory {
    const ID: &'static str = "memory";
    const DISPLAY_NAME: MetadataSource = MetadataSource::Explicit("Memory");
    const DESCRIPTION: MetadataSource =
        MetadataSource::Explicit("Stores and recalls text across tool calls");
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::required(&[capabilities::state::ACCESS.need()]);
    type Settings = Settings;

    fn new(_settings: Self::Settings) -> Self {
        Self
    }
}

impl Tools for Memory {
    fn definitions(&self) -> Result<Vec<ToolDefinition>, String> {
        Ok(vec![
            ToolDefinition {
                name: REMEMBER.to_owned(),
                description: "Store text under a key for later tool calls.".to_owned(),
                parameters: r#"{
                    "type":"object",
                    "properties":{
                        "key":{"type":"string","description":"Key used to store the value."},
                        "value":{"type":"string","description":"Text to remember."}
                    },
                    "required":["key","value"],
                    "additionalProperties":false
                }"#
                .to_owned(),
            },
            ToolDefinition {
                name: RECALL.to_owned(),
                description: "Recall text stored under a key by an earlier tool call.".to_owned(),
                parameters: r#"{
                    "type":"object",
                    "properties":{
                        "key":{"type":"string","description":"Key whose value should be recalled."}
                    },
                    "required":["key"],
                    "additionalProperties":false
                }"#
                .to_owned(),
            },
        ])
    }

    async fn execute(&self, name: String, arguments: String) -> Result<String, ToolError> {
        match name.as_str() {
            REMEMBER => {
                let RememberArguments { key, value } = parse_remember_arguments(&arguments)?;
                let byte_count = value.len();
                let output = format_stored(&key, byte_count);
                chap_plugin::state::set(key, value.into_bytes())
                    .await
                    .map(|()| output)
                    .map_err(Into::into)
            }
            RECALL => {
                let RecallArguments { key } = parse_recall_arguments(&arguments)?;
                let missing = format_missing(&key);
                chap_plugin::state::get(key)
                    .await
                    .map(|value| value.map(format_recalled).unwrap_or(missing))
                    .map_err(Into::into)
            }
            _ => Err(ToolError::InvalidInput(format!(
                "tool `{name}` is not provided by the memory plugin"
            ))),
        }
    }
}

fn parse_remember_arguments(arguments: &str) -> Result<RememberArguments, ToolError> {
    serde_json::from_str(arguments)
        .map_err(|error| ToolError::InvalidInput(format!("invalid `remember` arguments: {error}")))
}

fn parse_recall_arguments(arguments: &str) -> Result<RecallArguments, ToolError> {
    serde_json::from_str(arguments)
        .map_err(|error| ToolError::InvalidInput(format!("invalid `recall` arguments: {error}")))
}

fn format_stored(key: &str, byte_count: usize) -> String {
    format!("Stored {byte_count} bytes under `{key}`.")
}

fn format_recalled(value: Vec<u8>) -> String {
    String::from_utf8_lossy(&value).into_owned()
}

fn format_missing(key: &str) -> String {
    format!("Nothing is stored under `{key}`.")
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Settings {}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct RememberArguments {
    key: String,
    value: String,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct RecallArguments {
    key: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publishes_the_settings_object_schema() {
        let schema = chap_plugin::__private::settings_schema::<Memory>();
        let schema: serde_json::Value = serde_json::from_str(&schema).unwrap();

        assert_eq!(schema["type"], "object");
        assert_eq!(schema["unevaluatedProperties"], false);
    }

    #[test]
    fn parses_remember_arguments_and_rejects_unknown_fields() {
        assert_eq!(
            parse_remember_arguments(
                r#"{"key":"deployment-note","value":"Promote build 1042 after smoke tests."}"#
            )
            .unwrap(),
            RememberArguments {
                key: "deployment-note".to_owned(),
                value: "Promote build 1042 after smoke tests.".to_owned(),
            }
        );
        assert!(matches!(
            parse_remember_arguments(
                r#"{"key":"deployment-note","value":"ready","expires":"tomorrow"}"#
            ),
            Err(ToolError::InvalidInput(message))
                if message.starts_with("invalid `remember` arguments:")
        ));
    }

    #[test]
    fn parses_recall_arguments_and_rejects_unknown_fields() {
        assert_eq!(
            parse_recall_arguments(r#"{"key":"deployment-note"}"#).unwrap(),
            RecallArguments {
                key: "deployment-note".to_owned(),
            }
        );
        assert!(matches!(
            parse_recall_arguments(r#"{"key":"deployment-note","fallback":"unknown"}"#),
            Err(ToolError::InvalidInput(message))
                if message.starts_with("invalid `recall` arguments:")
        ));
    }

    #[test]
    fn publishes_both_tool_definitions_with_valid_parameter_schemas() {
        let definitions = <Memory as Tools>::definitions(&Memory).unwrap();

        assert_eq!(definitions.len(), 2);
        assert_eq!(definitions[0].name, REMEMBER);
        assert_eq!(definitions[1].name, RECALL);
        for definition in definitions {
            let parameters: serde_json::Value =
                serde_json::from_str(&definition.parameters).unwrap();
            assert_eq!(parameters["type"], "object");
            assert_eq!(parameters["additionalProperties"], false);
        }
    }

    #[test]
    fn formats_stored_recalled_and_missing_values() {
        assert_eq!(
            format_stored("deployment-note", 37),
            "Stored 37 bytes under `deployment-note`."
        );
        assert_eq!(
            format_recalled(b"Promote build 1042".to_vec()),
            "Promote build 1042"
        );
        assert_eq!(
            format_missing("missing"),
            "Nothing is stored under `missing`."
        );
    }

    #[test]
    fn rejects_a_tool_name_the_plugin_does_not_provide() {
        let error = futures::executor::block_on(<Memory as Tools>::execute(
            &Memory,
            "forget".to_owned(),
            "{}".to_owned(),
        ))
        .unwrap_err();

        assert_eq!(
            error,
            ToolError::InvalidInput(
                "tool `forget` is not provided by the memory plugin".to_owned()
            )
        );
    }
}

chap_plugin::plugin!(Memory: Tools, imports: State);
