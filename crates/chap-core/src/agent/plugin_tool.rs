use super::{InnerHost, PLUGIN_FUEL_PER_CALL, bindings};
use crate::{ExecutionMode, Tool, ToolDefinition};
use lockgate::{InvocationCtx, PluginHandle};
use std::{future::Future, pin::Pin, sync::Arc};

use bindings::{
    tools as tool_bindings,
    types::{ExecutionMode as BindingExecutionMode, ToolDefinition as BindingToolDefinition},
};

pub(super) struct PluginTool {
    plugin: String,
    handle: PluginHandle,
    definition: ToolDefinition,
    execution_mode: ExecutionMode,
    runtime: Arc<InnerHost>,
}

impl PluginTool {
    pub(super) async fn load(
        plugin: &str,
        runtime: Arc<InnerHost>,
        handle: PluginHandle,
        configured_mode: ExecutionMode,
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
            .map(|registration| {
                let declared_mode = registration.execution_mode.into();
                Self {
                    plugin: plugin.to_owned(),
                    handle: handle.clone(),
                    definition: registration.definition.into(),
                    execution_mode: resolve_tool_mode(declared_mode, configured_mode),
                    runtime: Arc::clone(&runtime),
                }
            })
            .collect())
    }
}

impl Tool for PluginTool {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }

    fn execution_mode(&self) -> ExecutionMode {
        self.execution_mode
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

fn resolve_tool_mode(
    declared_mode: ExecutionMode,
    configured_mode: ExecutionMode,
) -> ExecutionMode {
    if declared_mode == ExecutionMode::Sequential || configured_mode == ExecutionMode::Sequential {
        ExecutionMode::Sequential
    } else {
        ExecutionMode::Parallel
    }
}

impl From<BindingToolDefinition> for ToolDefinition {
    fn from(definition: BindingToolDefinition) -> Self {
        Self {
            name: definition.name,
            description: definition.description,
            parameters: definition.parameters,
        }
    }
}

impl From<BindingExecutionMode> for ExecutionMode {
    fn from(mode: BindingExecutionMode) -> Self {
        match mode {
            BindingExecutionMode::Parallel => Self::Parallel,
            BindingExecutionMode::Sequential => Self::Sequential,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_override_does_not_loosen_sequential_plugin_tools() {
        assert_eq!(
            resolve_tool_mode(ExecutionMode::Sequential, ExecutionMode::Parallel),
            ExecutionMode::Sequential
        );
    }
}
