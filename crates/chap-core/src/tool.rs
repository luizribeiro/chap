use std::{collections::BTreeMap, future::Future, pin::Pin, sync::Arc};

use serde::Deserialize;
use thiserror::Error;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionMode {
    #[default]
    Parallel,
    Sequential,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    /// A JSON Schema describing the tool's arguments.
    pub parameters: String,
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ToolRegistrationError {
    #[error("tool name cannot be empty")]
    EmptyName,
    #[error("tool `{name}` has invalid JSON Schema: {source}")]
    InvalidParameters {
        name: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("tool `{name}` is already registered")]
    DuplicateName { name: String },
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum ToolError {
    #[error("{0}")]
    InvalidInput(String),
    #[error("{0}")]
    Denied(String),
    #[error("{0}")]
    Failed(String),
    #[error("{0}")]
    Fatal(String),
}

pub trait Tool: Send + Sync {
    fn definition(&self) -> ToolDefinition;

    fn execution_mode(&self) -> ExecutionMode {
        ExecutionMode::default()
    }

    fn execute(
        &self,
        arguments: String,
    ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + '_>>;
}

pub(crate) struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub(crate) fn new() -> Self {
        Self {
            tools: BTreeMap::new(),
        }
    }

    pub(crate) fn register<T>(&mut self, tool: T) -> Result<(), ToolRegistrationError>
    where
        T: Tool + 'static,
    {
        let definition = tool.definition();
        validate(&definition)?;
        if self.tools.contains_key(&definition.name) {
            return Err(ToolRegistrationError::DuplicateName {
                name: definition.name,
            });
        }
        self.tools.insert(definition.name, Arc::new(tool));
        Ok(())
    }

    pub(crate) fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|tool| tool.definition()).collect()
    }

    pub(crate) fn execution_mode(&self, name: &str) -> ExecutionMode {
        self.tools
            .get(name)
            .map_or(ExecutionMode::default(), |tool| tool.execution_mode())
    }

    pub(crate) async fn execute(&self, name: &str, arguments: String) -> Result<String, ToolError> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| ToolError::Failed(format!("tool `{name}` is not registered")))?;
        tool.execute(arguments).await
    }
}

fn validate(definition: &ToolDefinition) -> Result<(), ToolRegistrationError> {
    if definition.name.trim().is_empty() {
        return Err(ToolRegistrationError::EmptyName);
    }
    serde_json::from_str::<serde_json::Value>(&definition.parameters).map_err(|source| {
        ToolRegistrationError::InvalidParameters {
            name: definition.name.clone(),
            source,
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestTool {
        name: &'static str,
        parameters: &'static str,
    }

    impl Tool for TestTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: self.name.to_owned(),
                description: "A test tool".to_owned(),
                parameters: self.parameters.to_owned(),
            }
        }

        fn execute(
            &self,
            arguments: String,
        ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + '_>> {
            Box::pin(async move { Ok(arguments) })
        }
    }

    struct SequentialTool;

    impl Tool for SequentialTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: "sequential".to_owned(),
                description: "A sequential test tool".to_owned(),
                parameters: r#"{"type":"object"}"#.to_owned(),
            }
        }

        fn execution_mode(&self) -> ExecutionMode {
            ExecutionMode::Sequential
        }

        fn execute(
            &self,
            arguments: String,
        ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + '_>> {
            Box::pin(async move { Ok(arguments) })
        }
    }

    #[test]
    fn registers_tools_by_name() {
        let mut registry = ToolRegistry::new();
        registry
            .register(TestTool {
                name: "echo",
                parameters: r#"{"type":"object"}"#,
            })
            .unwrap();

        assert_eq!(registry.definitions()[0].name, "echo");
    }

    #[test]
    fn rejects_duplicate_tool_names() {
        let mut registry = ToolRegistry::new();
        registry
            .register(TestTool {
                name: "echo",
                parameters: r#"{"type":"object"}"#,
            })
            .unwrap();
        let error = registry
            .register(TestTool {
                name: "echo",
                parameters: r#"{"type":"object"}"#,
            })
            .unwrap_err();

        assert!(matches!(
            error,
            ToolRegistrationError::DuplicateName { name } if name == "echo"
        ));
    }

    #[test]
    fn rejects_empty_tool_names() {
        let mut registry = ToolRegistry::new();
        let error = registry
            .register(TestTool {
                name: " ",
                parameters: r#"{"type":"object"}"#,
            })
            .unwrap_err();

        assert!(matches!(error, ToolRegistrationError::EmptyName));
    }

    #[test]
    fn rejects_invalid_parameter_schemas() {
        let mut registry = ToolRegistry::new();
        let error = registry
            .register(TestTool {
                name: "broken",
                parameters: "not JSON",
            })
            .unwrap_err();

        assert!(matches!(
            error,
            ToolRegistrationError::InvalidParameters { name, .. } if name == "broken"
        ));
    }

    #[test]
    fn defaults_tool_execution_to_parallel() {
        let mut registry = ToolRegistry::new();
        registry
            .register(TestTool {
                name: "echo",
                parameters: r#"{"type":"object"}"#,
            })
            .unwrap();

        assert_eq!(registry.execution_mode("echo"), ExecutionMode::Parallel);
    }

    #[test]
    fn reports_sequential_tool_execution() {
        let mut registry = ToolRegistry::new();
        registry.register(SequentialTool).unwrap();

        assert_eq!(
            registry.execution_mode("sequential"),
            ExecutionMode::Sequential
        );
    }

    #[test]
    fn defaults_unknown_tool_execution_to_parallel() {
        let registry = ToolRegistry::new();

        assert_eq!(registry.execution_mode("unknown"), ExecutionMode::Parallel);
    }

    #[tokio::test]
    async fn reports_unknown_tools_as_failed_executions() {
        let registry = ToolRegistry::new();

        assert_eq!(
            registry.execute("unknown", "{}".to_owned()).await,
            Err(ToolError::Failed(
                "tool `unknown` is not registered".to_owned()
            ))
        );
    }

    #[test]
    fn tool_errors_display_only_their_message() {
        let errors = [
            ToolError::InvalidInput("invalid".to_owned()),
            ToolError::Denied("denied".to_owned()),
            ToolError::Failed("failed".to_owned()),
            ToolError::Fatal("fatal".to_owned()),
        ];

        assert_eq!(
            errors.map(|error| error.to_string()),
            ["invalid", "denied", "failed", "fatal"]
        );
    }
}
