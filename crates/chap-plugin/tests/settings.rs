use chap_plugin as chap;
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
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
