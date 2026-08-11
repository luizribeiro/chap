use super::{ConfigurationComponent, InnerRuntime, host::Settings};
use jsonschema::error::ValidationErrorKind;
use serde_json::Value;
use std::path::Path;

const DRAFT_2020_12: &str = "https://json-schema.org/draft/2020-12/schema";

pub(super) async fn validate(
    runtime: &InnerRuntime,
    plugin: &str,
    component: ConfigurationComponent,
    settings: &Settings,
    source: &Path,
) -> Result<(), String> {
    let schema = runtime
        .component(component)
        .settings_schema()
        .await
        .map_err(|error| {
            format!("plugin `{plugin}` failed while publishing its settings schema: {error}")
        })?
        .map_err(|_| {
            format!("plugin `{plugin}` returned an error while publishing its settings schema")
        })?;
    validate_schema(plugin, &schema, settings.value(plugin)?, source)
}

fn validate_schema(
    plugin: &str,
    schema: &str,
    settings: &Value,
    source: &Path,
) -> Result<(), String> {
    let schema: Value = serde_json::from_str(schema).map_err(|error| {
        format!("plugin `{plugin}` returned malformed settings schema JSON: {error}")
    })?;

    if let Some(dialect) = schema.get("$schema") {
        let Some(dialect) = dialect.as_str() else {
            return Err(format!(
                "plugin `{plugin}` returned an invalid settings schema: `$schema` must be a string"
            ));
        };
        if dialect != DRAFT_2020_12 {
            return Err(format!(
                "plugin `{plugin}` declares an unsupported JSON Schema dialect; expected Draft 2020-12"
            ));
        }
    }

    jsonschema::draft202012::meta::validate(&schema).map_err(|_| {
        format!("plugin `{plugin}` returned an invalid Draft 2020-12 settings schema")
    })?;
    let validator = jsonschema::draft202012::new(&schema).map_err(|_| {
        format!("failed to compile settings schema for plugin `{plugin}` as Draft 2020-12")
    })?;
    if let Some(error) = validator.iter_errors(settings).next() {
        let (property, message) = match error.kind() {
            ValidationErrorKind::Required { property } => {
                (property.as_str(), "required setting is missing".to_owned())
            }
            ValidationErrorKind::AdditionalProperties { unexpected }
            | ValidationErrorKind::UnevaluatedProperties { unexpected } => (
                unexpected.first().map(String::as_str),
                "setting is not allowed".to_owned(),
            ),
            ValidationErrorKind::Type { .. } => (None, "value has the wrong type".to_owned()),
            kind => (
                None,
                format!("value does not satisfy schema rule `{}`", kind.keyword()),
            ),
        };
        let path = toml_path(plugin, settings, error.instance_path().as_str(), property);
        return Err(format!("{}: {path}: {message}", source.display()));
    }

    Ok(())
}

fn toml_path(plugin: &str, settings: &Value, pointer: &str, property: Option<&str>) -> String {
    let mut path = format!("plugins.{}.settings", toml_key(plugin));
    let mut current = settings;

    if !pointer.is_empty() {
        debug_assert!(pointer.starts_with('/'));
        for encoded in pointer.trim_start_matches('/').split('/') {
            let token = encoded.replace("~1", "/").replace("~0", "~");
            match current {
                Value::Array(values) => {
                    if let Ok(index) = token.parse::<usize>() {
                        path.push_str(&format!("[{index}]"));
                        current = values.get(index).unwrap_or(&Value::Null);
                    } else {
                        push_key(&mut path, &token);
                        current = &Value::Null;
                    }
                }
                Value::Object(values) => {
                    push_key(&mut path, &token);
                    current = values.get(&token).unwrap_or(&Value::Null);
                }
                _ => {
                    push_key(&mut path, &token);
                    current = &Value::Null;
                }
            }
        }
    }

    if let Some(property) = property {
        push_key(&mut path, property);
    }
    path
}

fn push_key(path: &mut String, key: &str) {
    path.push('.');
    path.push_str(&toml_key(key));
}

fn toml_key(key: &str) -> String {
    if !key.is_empty()
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        key.to_owned()
    } else {
        serde_json::to_string(key).expect("serializing a string cannot fail")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn converts_json_pointers_to_toml_paths() {
        let settings = json!({"backends": [{"url": "https://example.com"}]});

        assert_eq!(
            toml_path("openai", &settings, "/backends/0/url", None),
            "plugins.openai.settings.backends[0].url"
        );
    }

    #[test]
    fn quotes_plugin_ids_and_setting_keys_when_toml_requires_it() {
        let settings = json!({"nested/key": {"space key": true}});

        assert_eq!(
            toml_path(
                "example.provider",
                &settings,
                "/nested~1key/space key",
                None
            ),
            "plugins.\"example.provider\".settings.\"nested/key\".\"space key\""
        );
    }

    #[test]
    fn appends_properties_reported_at_the_parent_object() {
        assert_eq!(
            toml_path("example", &json!({}), "", Some("required.value")),
            "plugins.example.settings.\"required.value\""
        );
    }

    #[test]
    fn treats_numeric_object_keys_as_keys_instead_of_array_indexes() {
        assert_eq!(
            toml_path("example", &json!({"0": true}), "/0", None),
            "plugins.example.settings.0"
        );
    }

    #[test]
    fn accepts_valid_settings() {
        validate_schema(
            "example",
            &schema(r#"{"model":{"type":"string"}}"#, r#"["model"]"#),
            &json!({"model": "sage-1"}),
            Path::new("sage.toml"),
        )
        .unwrap();
    }

    #[test]
    fn reports_missing_required_properties_at_the_toml_path() {
        let error = validate_schema(
            "example.provider",
            &schema(r#"{"model":{"type":"string"}}"#, r#"["model"]"#),
            &json!({}),
            Path::new("config/sage.toml"),
        )
        .unwrap_err();

        assert_eq!(
            error,
            "config/sage.toml: plugins.\"example.provider\".settings.model: required setting is missing"
        );
    }

    #[test]
    fn reports_wrong_types_without_printing_the_value() {
        let error = validate_schema(
            "example",
            &schema(r#"{"api-key":{"type":"integer"}}"#, "[]"),
            &json!({"api-key": "do-not-print-this-secret"}),
            Path::new("sage.toml"),
        )
        .unwrap_err();

        assert_eq!(
            error,
            "sage.toml: plugins.example.settings.api-key: value has the wrong type"
        );
        assert!(!error.contains("do-not-print-this-secret"));
    }

    #[test]
    fn reports_unknown_properties_at_their_toml_path() {
        let error = validate_schema(
            "example",
            &schema(r#"{"known":{"type":"boolean"}}"#, "[]"),
            &json!({"unknown.key": true}),
            Path::new("sage.toml"),
        )
        .unwrap_err();

        assert_eq!(
            error,
            "sage.toml: plugins.example.settings.\"unknown.key\": setting is not allowed"
        );
    }

    #[test]
    fn rejects_malformed_schema_json() {
        let error =
            validate_schema("example", "{", &json!({}), Path::new("sage.toml")).unwrap_err();

        assert!(error.contains("plugin `example` returned malformed settings schema JSON"));
    }

    #[test]
    fn rejects_schemas_from_an_unsupported_dialect() {
        let error = validate_schema(
            "example",
            r#"{"$schema":"http://json-schema.org/draft-07/schema#"}"#,
            &json!({}),
            Path::new("sage.toml"),
        )
        .unwrap_err();

        assert!(error.contains("declares an unsupported JSON Schema dialect"));
    }

    #[test]
    fn rejects_schemas_that_fail_draft_2020_12_meta_validation() {
        let error = validate_schema(
            "example",
            r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"invalid"}"#,
            &json!({}),
            Path::new("sage.toml"),
        )
        .unwrap_err();

        assert!(error.contains("invalid Draft 2020-12 settings schema"));
    }

    #[test]
    fn rejects_schemas_that_cannot_be_compiled() {
        let error = validate_schema(
            "example",
            r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","$ref":"urn:missing:schema"}"#,
            &json!({}),
            Path::new("sage.toml"),
        )
        .unwrap_err();

        assert!(error.contains("failed to compile settings schema"));
    }

    fn schema(properties: &str, required: &str) -> String {
        format!(
            r#"{{"$schema":"{DRAFT_2020_12}","type":"object","properties":{properties},"required":{required},"additionalProperties":false}}"#
        )
    }
}
