use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    plugins: BTreeMap<String, Plugin>,
    #[serde(skip)]
    directory: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plugin {
    component: PathBuf,
    #[serde(default)]
    settings: toml::Table,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, String> {
        let source = fs::read_to_string(path)
            .map_err(|error| format!("failed to read `{}`: {error}", path.display()))?;
        let mut config: Self = toml::from_str(&source)
            .map_err(|error| format!("failed to parse `{}`: {error}", path.display()))?;
        config.directory = path.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
        Ok(config)
    }

    pub fn plugins(&self) -> impl Iterator<Item = (&str, &Plugin)> {
        self.plugins
            .iter()
            .map(|(id, plugin)| (id.as_str(), plugin))
    }

    pub(crate) fn plugin(&self, id: &str) -> Option<&Plugin> {
        self.plugins.get(id)
    }

    pub(crate) fn component_path(&self, plugin: &Plugin) -> PathBuf {
        self.directory.join(&plugin.component)
    }
}

impl Plugin {
    pub fn component(&self) -> &Path {
        &self.component
    }

    pub(crate) fn settings(&self) -> &toml::Table {
        &self.settings
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

        let (id, plugin) = config.plugins().next().unwrap();
        assert_eq!(id, "openai");
        assert_eq!(
            plugin.component(),
            Path::new("./plugins/openai-compatible.wasm")
        );
        assert_eq!(plugin.settings()["model"].as_str(), Some("example-model"));
    }
}
