use chap_plugin as chap;

#[derive(Debug, PartialEq, chap::Settings)]
struct Settings {
    base_url: String,
    egress_origin: String,
    model: String,
    api_key: Option<String>,
}

#[derive(Debug, PartialEq, chap::Settings)]
struct AttributedSettings {
    /// Provider deployment identifier.
    #[serde(rename = "deployment")]
    #[schemars(length(max = 40))]
    model: String,
    #[serde(default)]
    retries: u32,
}

struct TestPlugin;

impl chap::Plugin for TestPlugin {
    const ID: &'static str = "settings-test";
    const NEEDS: chap::Needs = chap::Needs::NOTHING;
    type Settings = Settings;

    fn new(_settings: Self::Settings) -> Self {
        Self
    }
}

impl chap::__lockgate::Plugin for TestPlugin {
    const ID: &'static str = <Self as chap::Plugin>::ID;
    const NEEDS: chap::Needs = <Self as chap::Plugin>::NEEDS;
    type Settings = <Self as chap::Plugin>::Settings;
}

struct AttributedTestPlugin;

impl chap::Plugin for AttributedTestPlugin {
    const ID: &'static str = "attributed-settings-test";
    const NEEDS: chap::Needs = chap::Needs::NOTHING;
    type Settings = AttributedSettings;

    fn new(_settings: Self::Settings) -> Self {
        Self
    }
}

impl chap::__lockgate::Plugin for AttributedTestPlugin {
    const ID: &'static str = <Self as chap::Plugin>::ID;
    const NEEDS: chap::Needs = <Self as chap::Plugin>::NEEDS;
    type Settings = <Self as chap::Plugin>::Settings;
}

#[test]
fn deserializes_kebab_case_with_optional_values() {
    let with_api_key: Settings = serde_json::from_str(
        r#"{
            "base-url": "https://example.com/v1",
            "egress-origin": "https://example.com",
            "model": "example-model",
            "api-key": "secret"
        }"#,
    )
    .unwrap();
    assert_eq!(with_api_key.api_key.as_deref(), Some("secret"));

    let without_api_key: Settings = serde_json::from_str(
        r#"{
            "base-url": "https://example.com/v1",
            "egress-origin": "https://example.com",
            "model": "example-model"
        }"#,
    )
    .unwrap();
    assert_eq!(without_api_key.api_key, None);
}

#[test]
fn rejects_snake_case_and_unknown_fields() {
    let snake_case = r#"{
        "base_url": "https://example.com/v1",
        "egress-origin": "https://example.com",
        "model": "example-model"
    }"#;
    assert!(serde_json::from_str::<Settings>(snake_case).is_err());

    let unknown = r#"{
        "base-url": "https://example.com/v1",
        "egress-origin": "https://example.com",
        "model": "example-model",
        "surprise": true
    }"#;
    assert!(serde_json::from_str::<Settings>(unknown).is_err());
}

#[test]
fn forwards_serde_field_attributes() {
    let settings: AttributedSettings = serde_json::from_str(
        r#"{
            "deployment": "example-model"
        }"#,
    )
    .unwrap();

    assert_eq!(settings.model, "example-model");
    assert_eq!(settings.retries, 0);
}

#[test]
fn forwards_field_schema_attributes_and_docs() {
    let schema: serde_json::Value =
        serde_json::from_str(&chap::__private::settings_schema::<AttributedTestPlugin>()).unwrap();
    let deployment = &schema["properties"]["deployment"];

    assert_eq!(deployment["description"], "Provider deployment identifier.");
    assert_eq!(deployment["maxLength"], 40);
    assert_eq!(deployment["pattern"], r"\S");
}

#[test]
fn lockgate_schema_preserves_chap_settings_conventions() {
    fn assert_plugin_settings<T: chap::serde::de::DeserializeOwned + chap::schemars::JsonSchema>() {
    }
    assert_plugin_settings::<Settings>();

    let schema: serde_json::Value =
        serde_json::from_str(&chap::__private::settings_schema::<TestPlugin>()).unwrap();
    let required = schema["required"].as_array().unwrap();
    for name in ["base-url", "egress-origin", "model"] {
        assert!(required.iter().any(|value| value == name), "{schema:#}");
        assert_eq!(schema["properties"][name]["pattern"], r"\S");
    }
    assert!(!required.iter().any(|value| value == "api-key"));
    assert!(schema["properties"]["api-key"].get("pattern").is_none());
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["unevaluatedProperties"], false);
}
