use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[expect(dead_code)]
struct Settings {
    #[schemars(regex(pattern = r"\S"))]
    base_url: String,
    #[schemars(regex(pattern = r"\S"))]
    egress_origin: String,
    #[schemars(regex(pattern = r"\S"))]
    model: String,
    api_key: Option<String>,
}

struct TestPlugin;

impl chap_plugin::Plugin for TestPlugin {
    const ID: &'static str = "settings-test";
    const NEEDS: chap_plugin::Needs = chap_plugin::Needs::NOTHING;
    type Settings = Settings;

    fn new(_settings: Self::Settings) -> Self {
        Self
    }
}

impl chap_plugin::__lockgate::Plugin for TestPlugin {
    const ID: &'static str = <Self as chap_plugin::Plugin>::ID;
    const NEEDS: chap_plugin::Needs = <Self as chap_plugin::Plugin>::NEEDS;
    type Settings = <Self as chap_plugin::Plugin>::Settings;
}

#[test]
fn lockgate_schema_preserves_chap_settings_conventions() {
    fn assert_plugin_settings<T: serde::de::DeserializeOwned + schemars::JsonSchema>() {}
    assert_plugin_settings::<Settings>();

    let schema: serde_json::Value =
        serde_json::from_str(&chap_plugin::__private::settings_schema::<TestPlugin>()).unwrap();
    let required = schema["required"].as_array().unwrap();
    for name in ["base_url", "egress_origin", "model"] {
        assert!(required.iter().any(|value| value == name), "{schema:#}");
        assert_eq!(schema["properties"][name]["pattern"], r"\S");
    }
    assert!(!required.iter().any(|value| value == "api_key"));
    assert!(schema["properties"]["api_key"].get("pattern").is_none());
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["unevaluatedProperties"], false);
}
