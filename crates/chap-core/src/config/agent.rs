use super::Config;
use crate::tool::ExecutionMode;
use serde::Deserialize;
use std::num::NonZeroUsize;

/// Settings that control agent-wide behavior.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentSettings {
    /// How tool calls are scheduled across the agent.
    #[serde(default)]
    tool_execution: ToolExecutionSettings,
}

/// Settings that control tool-call scheduling.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolExecutionSettings {
    /// How batches of tool calls are scheduled.
    #[serde(default)]
    pub(crate) mode: ExecutionMode,
    /// Maximum number of tool calls allowed in flight.
    #[serde(default = "default_max_concurrency")]
    pub(crate) max_concurrency: NonZeroUsize,
}

impl Config {
    pub(crate) fn tool_execution(&self) -> ToolExecutionSettings {
        self.agent.tool_execution
    }
}

impl Default for ToolExecutionSettings {
    fn default() -> Self {
        Self {
            mode: ExecutionMode::default(),
            max_concurrency: default_max_concurrency(),
        }
    }
}

fn default_max_concurrency() -> NonZeroUsize {
    NonZeroUsize::new(8).expect("default tool concurrency is nonzero")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_agent_tool_execution_when_sections_are_absent() {
        for source in [
            "{}",
            r#"{ "agent": {} }"#,
            r#"{ "agent": { "tool_execution": {} } }"#,
        ] {
            let config: Config = serde_json::from_str(source).unwrap();

            assert_eq!(config.agent.tool_execution.mode, ExecutionMode::Parallel);
            assert_eq!(config.agent.tool_execution.max_concurrency.get(), 8);
        }
    }

    #[test]
    fn parses_sequential_agent_tool_execution() {
        let config: Config = serde_json::from_str(
            r#"{
                "agent": {
                    "tool_execution": {
                        "mode": "sequential"
                    }
                }
            }"#,
        )
        .unwrap();

        assert_eq!(config.agent.tool_execution.mode, ExecutionMode::Sequential);
    }

    #[test]
    fn rejects_zero_agent_tool_concurrency() {
        let error = serde_json::from_str::<Config>(
            r#"{
                "agent": {
                    "tool_execution": {
                        "max_concurrency": 0
                    }
                }
            }"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("nonzero usize"));
    }

    #[test]
    fn rejects_unknown_agent_tool_execution_settings() {
        let error = serde_json::from_str::<Config>(
            r#"{
                "agent": {
                    "tool_execution": {
                        "concurrency": 8
                    }
                }
            }"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("unknown field `concurrency`"));
    }
}
