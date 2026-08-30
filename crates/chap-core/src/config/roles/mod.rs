mod context;
mod tools;

pub(crate) use context::{ContextChannel, ContextSettings};
pub(crate) use tools::ToolsSettings;

use super::ConfiguredPlugin;

/// Resolved role settings carried with one loaded plugin.
#[derive(Clone, Debug, Default)]
pub(crate) struct PluginRoleSettings {
    pub(crate) tools: ToolsSettings,
    pub(crate) context: ContextSettings,
}

impl ConfiguredPlugin {
    pub(crate) fn role_settings(&self) -> PluginRoleSettings {
        PluginRoleSettings {
            tools: self.tools.clone().unwrap_or_default(),
            context: self.context.clone().unwrap_or_default(),
        }
    }
}
