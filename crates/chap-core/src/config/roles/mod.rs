mod context;
mod tools;

pub(crate) use context::{ContextChannel, ContextSettings};
pub(crate) use tools::ToolsSettings;

use super::ConfiguredPlugin;

/// Resolved role settings carried with one loaded plugin.
#[derive(Clone, Debug, Default)]
pub(crate) struct PluginRoleSettings {
    tools: ToolsSettings,
    context: ContextSettings,
}

impl PluginRoleSettings {
    pub(crate) fn tools(&self) -> &ToolsSettings {
        &self.tools
    }

    pub(crate) fn context(&self) -> &ContextSettings {
        &self.context
    }
}

impl ConfiguredPlugin {
    pub(crate) fn role_settings(&self) -> PluginRoleSettings {
        PluginRoleSettings {
            tools: self.tools.clone().unwrap_or_default(),
            context: self.context.clone().unwrap_or_default(),
        }
    }
}
