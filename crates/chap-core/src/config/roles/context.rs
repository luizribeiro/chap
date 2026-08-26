use super::super::ConfiguredPlugin;
use serde::Deserialize;

/// Destination for context contributed by a context-role plugin.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ContextChannel {
    /// Contributed context is added as user context.
    #[default]
    Context,
    /// Contributed context is added as operator instructions.
    System,
}

/// Settings for one plugin acting in the context role.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContextSettings {
    /// Where the plugin's contributed context is placed.
    #[serde(default)]
    channel: ContextChannel,
}

impl ConfiguredPlugin {
    pub(crate) fn context_channel(&self) -> ContextChannel {
        self.context
            .as_ref()
            .map(|context| context.channel)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn parses_plugin_context_channels_and_defaults_to_context() {
        let config: Config = serde_json::from_str(
            r#"{
                "plugins": {
                    "default-context": {
                        "component": "context.wasm"
                    },
                    "operator-context": {
                        "component": "context.wasm",
                        "context": {
                            "channel": "system"
                        }
                    }
                }
            }"#,
        )
        .unwrap();

        assert_eq!(
            config.plugin("default-context").unwrap().context_channel(),
            ContextChannel::Context
        );
        assert_eq!(
            config.plugin("operator-context").unwrap().context_channel(),
            ContextChannel::System
        );
    }

    #[test]
    fn rejects_unknown_plugin_context_settings() {
        for context in [
            r#"{ "channel": "assistant" }"#,
            r#"{ "channel": "context", "wrap": true }"#,
        ] {
            let source = format!(
                r#"{{
                    "plugins": {{
                        "example": {{
                            "component": "context.wasm",
                            "context": {context}
                        }}
                    }}
                }}"#
            );

            assert!(serde_json::from_str::<Config>(&source).is_err());
        }
    }
}
