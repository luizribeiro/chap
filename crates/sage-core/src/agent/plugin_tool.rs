use super::{InnerRuntime, ToolComponent, bindings};
use crate::{Tool, ToolDefinition};
use std::{future::Future, pin::Pin, sync::Arc};

use bindings::__lockgate_world_1::exports::sage::agent::tools as tool_bindings;

pub(super) struct PluginTool {
    plugin: String,
    component: ToolComponent,
    definition: ToolDefinition,
    runtime: Arc<InnerRuntime>,
}

impl PluginTool {
    pub(super) async fn load(
        plugin: &str,
        runtime: Arc<InnerRuntime>,
        component: ToolComponent,
    ) -> Result<Vec<Self>, String> {
        let definitions = runtime
            .component(component)
            .definitions()
            .await
            .map_err(|error| format!("tool plugin `{plugin}` failed: {error}"))?
            .map_err(|error| format!("tool plugin `{plugin}`: {error}"))?;
        Ok(definitions
            .into_iter()
            .map(|definition| Self {
                plugin: plugin.to_owned(),
                component,
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
                .component(self.component)
                .execute(self.definition.name.clone(), arguments)
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
