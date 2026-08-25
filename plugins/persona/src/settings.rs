use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Settings {
    #[schemars(regex(pattern = r"\S"))]
    pub(crate) persona: String,
    #[serde(default)]
    pub(crate) priority: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_settings_and_defaults_priority() {
        let defaulted: Settings =
            serde_json::from_value(serde_json::json!({ "persona": "Be concise." })).unwrap();
        assert_eq!(defaulted.persona, "Be concise.");
        assert_eq!(defaulted.priority, 0);

        let configured: Settings = serde_json::from_value(serde_json::json!({
            "persona": "Be expansive.",
            "priority": -12,
        }))
        .unwrap();
        assert_eq!(configured.priority, -12);
    }

    #[test]
    fn schema_rejects_empty_or_whitespace_only_personas() {
        let schema = schemars::generate::SchemaSettings::draft2020_12()
            .for_deserialize()
            .into_generator()
            .into_root_schema_for::<Settings>();
        let schema = serde_json::to_value(schema).unwrap();
        let validator = jsonschema::draft202012::options().build(&schema).unwrap();

        for persona in ["", " ", "\t\n"] {
            assert!(
                validator
                    .validate(&serde_json::json!({ "persona": persona }))
                    .is_err(),
                "schema accepted blank persona {persona:?}"
            );
        }
        assert!(
            validator
                .validate(&serde_json::json!({ "persona": "Be helpful." }))
                .is_ok()
        );
    }
}
