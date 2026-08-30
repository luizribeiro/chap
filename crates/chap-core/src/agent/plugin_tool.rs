use super::{CallBudgets, InnerHost, PluginCall, StartError, bindings};
use crate::{ExecutionMode, Tool, ToolDefinition, ToolError, config::roles::ToolsSettings};
use lockgate::{CallError, PluginHandle};
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
    call_budgets: CallBudgets,
}

impl PluginTool {
    pub(super) async fn load(
        plugin: &str,
        runtime: Arc<InnerHost>,
        handle: PluginHandle,
        settings: &ToolsSettings,
        call_budgets: CallBudgets,
    ) -> Result<Vec<Self>, StartError> {
        let definitions = runtime
            .client::<tool_bindings::Role>(&handle)
            .map_err(|source| StartError::ToolRole {
                plugin: plugin.to_owned(),
                source,
            })?
            .definitions(
                call_budgets
                    .resolve(PluginCall::ToolDefinitions)
                    .invocation_context(),
            )
            .await
            .map_err(|source| StartError::ToolDefinitionsCall {
                plugin: plugin.to_owned(),
                source,
            })?
            .map_err(|message| StartError::ToolDefinitions {
                plugin: plugin.to_owned(),
                message,
            })?;
        Ok(definitions
            .into_iter()
            .map(|registration| {
                let declared_mode = registration.execution_mode.into();
                Self {
                    plugin: plugin.to_owned(),
                    handle: handle.clone(),
                    definition: registration.definition.into(),
                    execution_mode: resolve_tool_mode(declared_mode, settings.execution()),
                    runtime: Arc::clone(&runtime),
                    call_budgets,
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
    ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + '_>> {
        Box::pin(async move {
            self.runtime
                .client::<tool_bindings::Role>(&self.handle)
                .map_err(|error| {
                    ToolError::Failed(format!("tool plugin `{}` failed: {error}", self.plugin))
                })?
                .execute(
                    self.call_budgets
                        .resolve(PluginCall::ToolExecute)
                        .invocation_context(),
                    &self.definition.name,
                    &arguments,
                )
                .await
                .map_err(|error| tool_call_error(&self.plugin, error))?
                .map_err(|error| map_tool_error(&self.plugin, error))
        })
    }
}

fn map_tool_error(plugin: &str, error: tool_bindings::ToolError) -> ToolError {
    let message = |detail| format!("tool plugin `{plugin}`: {detail}");
    match error {
        tool_bindings::ToolError::InvalidInput(error) => ToolError::InvalidInput(message(error)),
        tool_bindings::ToolError::Denied(error) => ToolError::Denied(message(error)),
        tool_bindings::ToolError::Failed(error) => ToolError::Failed(message(error)),
        tool_bindings::ToolError::Fatal(error) => ToolError::Fatal(message(error)),
    }
}

fn tool_call_error(plugin: &str, error: CallError) -> ToolError {
    ToolError::Failed(match error {
        CallError::DeadlineExceeded { deadline } => {
            format!("tool plugin `{plugin}` timed out after {deadline:?}")
        }
        error => format!("tool plugin `{plugin}` failed: {error}"),
    })
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
    fn maps_every_wire_tool_error_to_the_matching_host_variant() {
        let errors = [
            (
                tool_bindings::ToolError::InvalidInput(
                    "expected integer field `limit`, got a string".to_owned(),
                ),
                ToolError::InvalidInput(
                    "tool plugin `example`: expected integer field `limit`, got a string"
                        .to_owned(),
                ),
            ),
            (
                tool_bindings::ToolError::Denied(
                    "capability policy denied access to project files".to_owned(),
                ),
                ToolError::Denied(
                    "tool plugin `example`: capability policy denied access to project files"
                        .to_owned(),
                ),
            ),
            (
                tool_bindings::ToolError::Failed(
                    "command exited with status 17 after writing stderr".to_owned(),
                ),
                ToolError::Failed(
                    "tool plugin `example`: command exited with status 17 after writing stderr"
                        .to_owned(),
                ),
            ),
            (
                tool_bindings::ToolError::Fatal(
                    "plugin runtime could not load its configuration".to_owned(),
                ),
                ToolError::Fatal(
                    "tool plugin `example`: plugin runtime could not load its configuration"
                        .to_owned(),
                ),
            ),
        ];

        for (error, expected) in errors {
            assert_eq!(map_tool_error("example", error), expected);
        }
    }

    #[test]
    fn parallel_override_does_not_loosen_sequential_plugin_tools() {
        assert_eq!(
            resolve_tool_mode(ExecutionMode::Sequential, ExecutionMode::Parallel),
            ExecutionMode::Sequential
        );
    }
}
