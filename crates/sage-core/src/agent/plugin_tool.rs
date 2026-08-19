use super::{InnerHost, PLUGIN_FUEL_PER_CALL, bindings};
use crate::{Tool, ToolDefinition};
use lockgate::{InvocationCtx, PluginHandle};
use std::{future::Future, pin::Pin, sync::Arc};

use bindings::tools as tool_bindings;

pub(super) struct PluginTool {
    plugin: String,
    handle: PluginHandle,
    definition: ToolDefinition,
    runtime: Arc<InnerHost>,
}

impl PluginTool {
    pub(super) async fn load(
        plugin: &str,
        runtime: Arc<InnerHost>,
        handle: PluginHandle,
    ) -> Result<Vec<Self>, String> {
        let definitions = runtime
            .client::<tool_bindings::Role>(&handle)
            .map_err(|error| format!("tool plugin `{plugin}` failed: {error}"))?
            .definitions(InvocationCtx::bounded(PLUGIN_FUEL_PER_CALL))
            .await
            .map_err(|error| format!("tool plugin `{plugin}` failed: {error}"))?
            .map_err(|error| format!("tool plugin `{plugin}`: {error}"))?;
        Ok(definitions
            .into_iter()
            .map(|definition| Self {
                plugin: plugin.to_owned(),
                handle: handle.clone(),
                definition: definition.into(),
                runtime: Arc::clone(&runtime),
            })
            .collect())
    }
}

impl Tool for PluginTool {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }

    fn execute(
        &self,
        arguments: String,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
        Box::pin(async move {
            self.runtime
                .client::<tool_bindings::Role>(&self.handle)
                .map_err(|error| format!("tool plugin `{}` failed: {error}", self.plugin))?
                .execute(
                    InvocationCtx::bounded(PLUGIN_FUEL_PER_CALL),
                    &self.definition.name,
                    &arguments,
                )
                .await
                .map_err(|error| format!("tool plugin `{}` failed: {error}", self.plugin))?
                .map_err(|error| format!("tool plugin `{}`: {error}", self.plugin))
        })
    }
}

impl From<tool_bindings::ToolDefinition> for ToolDefinition {
    fn from(definition: tool_bindings::ToolDefinition) -> Self {
        Self {
            name: definition.name,
            description: definition.description,
            parameters: definition.parameters,
        }
    }
}
