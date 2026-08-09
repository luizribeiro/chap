use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    #[serde(default)]
    plugins: BTreeMap<String, Plugin>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Plugin {
    component: PathBuf,
    #[serde(default, rename = "settings")]
    _settings: toml::Table,
}

impl Config {
    pub(crate) fn load(path: &Path) -> Result<Self, String> {
        let source = fs::read_to_string(path)
            .map_err(|error| format!("failed to read `{}`: {error}", path.display()))?;
        toml::from_str(&source)
            .map_err(|error| format!("failed to parse `{}`: {error}", path.display()))
    }

    pub(crate) fn plugins(&self) -> impl Iterator<Item = (&str, &Path)> {
        self.plugins
            .iter()
            .map(|(id, plugin)| (id.as_str(), plugin.component.as_path()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plugins() {
        let config: Config = toml::from_str(
            r#"
[plugins.openai]
component = "./plugins/openai-compatible.wasm"

[plugins.openai.settings]
model = "example-model"
"#,
        )
        .unwrap();

        let (id, component) = config.plugins().next().unwrap();
        assert_eq!(id, "openai");
        assert_eq!(component, Path::new("./plugins/openai-compatible.wasm"));
        assert_eq!(
            config.plugins["openai"]._settings["model"].as_str(),
            Some("example-model")
        );
    }
}
