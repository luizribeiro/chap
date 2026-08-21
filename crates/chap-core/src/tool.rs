use std::{collections::BTreeMap, future::Future, pin::Pin, sync::Arc};

use serde::Deserialize;

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

pub trait Tool: Send + Sync {
    fn definition(&self) -> ToolDefinition;

    fn execution_mode(&self) -> ExecutionMode {
        ExecutionMode::default()
    }

    fn execute(
        &self,
        arguments: String,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>>;
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

    pub(crate) fn register<T>(&mut self, tool: T) -> Result<(), String>
    where
        T: Tool + 'static,
    {
        let definition = tool.definition();
        validate(&definition)?;
        if self.tools.contains_key(&definition.name) {
            return Err(format!("tool `{}` is already registered", definition.name));
        }
        self.tools.insert(definition.name, Arc::new(tool));
        Ok(())
    }

    pub(crate) fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|tool| tool.definition()).collect()
    }

    #[allow(dead_code)]
    pub(crate) fn execution_mode(&self, name: &str) -> ExecutionMode {
        self.tools
            .get(name)
            .map_or(ExecutionMode::default(), |tool| tool.execution_mode())
    }

    pub(crate) async fn execute(&self, name: &str, arguments: String) -> Result<String, String> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| format!("tool `{name}` is not registered"))?;
        tool.execute(arguments).await
    }
}

fn validate(definition: &ToolDefinition) -> Result<(), String> {
    if definition.name.trim().is_empty() {
        return Err("tool name cannot be empty".to_owned());
    }
    serde_json::from_str::<serde_json::Value>(&definition.parameters).map_err(|error| {
        format!(
            "tool `{}` has invalid JSON Schema: {error}",
            definition.name
        )
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
        ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
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
        ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
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

        assert_eq!(error, "tool `echo` is already registered");
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

        assert!(error.contains("tool `broken` has invalid JSON Schema"));
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
}
