use super::{Config, LoadError};
use crate::tool::ExecutionMode;
use serde::Deserialize;
use serde_json::Value;
use std::num::NonZeroUsize;

/// Settings that control agent-wide behavior.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentSettings {
    /// How tool calls are scheduled across the agent.
    #[serde(default)]
    tool_execution: ToolExecutionSettings,
    exec: Option<Value>,
    state: Option<Value>,
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

    pub(crate) fn validate_agent_settings(&self) -> Result<(), LoadError> {
        if self.agent.exec.is_some() && !cfg!(feature = "exec") {
            return Err(LoadError::CapabilityUnsupported { capability: "exec" });
        }

        #[cfg(feature = "exec")]
        self.exec_settings()?;

        if self.agent.state.is_some() && !cfg!(feature = "state") {
            return Err(LoadError::CapabilityUnsupported {
                capability: "state",
            });
        }

        #[cfg(feature = "state")]
        self.state_settings()?;

        Ok(())
    }

    #[cfg(feature = "exec")]
    pub(crate) fn exec_settings(&self) -> Result<chap_exec::host::ExecSettings, LoadError> {
        self.agent
            .exec
            .clone()
            .map(serde_json::from_value)
            .transpose()
            .map(Option::unwrap_or_default)
            .map_err(|source| LoadError::InvalidAgentConfigSection {
                section: "agent.exec",
                source,
            })
    }

    #[cfg(feature = "state")]
    pub(crate) fn state_settings(&self) -> Result<chap_state::host::StateSettings, LoadError> {
        self.agent
            .state
            .clone()
            .map(serde_json::from_value)
            .transpose()
            .map(Option::unwrap_or_default)
            .map_err(|source| LoadError::InvalidAgentConfigSection {
                section: "agent.state",
                source,
            })
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
    use crate::config::load_config;

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
        let error = load_config(
            r#"{
                "agent": {
                    "tool_execution": {
                        "max_concurrency": 0
                    }
                }
            }"#,
        )
        .unwrap_err();

        let LoadError::ParseConfig { source, .. } = error else {
            panic!("expected config parse failure");
        };
        assert!(source.to_string().contains("nonzero usize"));
    }

    #[test]
    fn rejects_unknown_agent_tool_execution_settings() {
        let error = load_config(
            r#"{
                "agent": {
                    "tool_execution": {
                        "concurrency": 8
                    }
                }
            }"#,
        )
        .unwrap_err();

        let LoadError::ParseConfig { source, .. } = error else {
            panic!("expected config parse failure");
        };
        assert!(source.to_string().contains("unknown field `concurrency`"));
    }

    #[test]
    fn accepts_an_absent_exec_section() {
        let config: Config = serde_json::from_str(r#"{ "agent": {} }"#).unwrap();

        assert!(config.agent.exec.is_none());
        config.validate_agent_settings().unwrap();
    }

    #[cfg(not(feature = "exec"))]
    #[test]
    fn rejects_exec_settings_when_exec_support_is_absent() {
        let config: Config = serde_json::from_str(
            r#"{
                "agent": {
                    "exec": { "timeout_ceiling_ms": 1000 }
                }
            }"#,
        )
        .unwrap();

        let error = config.validate_agent_settings().unwrap_err();
        assert!(matches!(
            error,
            LoadError::CapabilityUnsupported { capability: "exec" }
        ));
    }

    #[cfg(feature = "exec")]
    #[test]
    fn parses_exec_settings_without_teaching_core_the_schema() {
        let config: Config = serde_json::from_str(
            r#"{
                "agent": {
                    "exec": {
                        "path": ["/bin", "/usr/bin"],
                        "timeout_ceiling_ms": 1000
                    }
                }
            }"#,
        )
        .unwrap();

        let exec = config.exec_settings().unwrap();
        assert_eq!(
            exec.path.unwrap(),
            [
                std::path::PathBuf::from("/bin"),
                std::path::PathBuf::from("/usr/bin")
            ]
        );
        assert_eq!(exec.timeout_ceiling_ms, 1000);
    }

    #[cfg(feature = "exec")]
    #[test]
    fn defaults_exec_settings_when_the_section_is_absent() {
        let config: Config = serde_json::from_str("{}").unwrap();
        let exec = config.exec_settings().unwrap();

        assert!(exec.path.is_none());
        assert_eq!(exec.timeout_ceiling_ms, 120_000);
    }

    #[cfg(feature = "exec")]
    #[test]
    fn names_the_agent_exec_section_when_settings_are_invalid() {
        let config: Config = serde_json::from_str(
            r#"{
                "agent": {
                    "exec": { "timeout_ceiling_ms": "soon" }
                }
            }"#,
        )
        .unwrap();

        let error = config.validate_agent_settings().unwrap_err();
        let LoadError::InvalidAgentConfigSection {
            section: "agent.exec",
            source,
        } = error
        else {
            panic!("expected invalid exec config");
        };
        assert!(source.to_string().contains("invalid type"));
    }

    #[cfg(not(feature = "state"))]
    #[test]
    fn rejects_state_settings_when_state_support_is_absent() {
        let config: Config = serde_json::from_str(
            r#"{
                "agent": {
                    "state": { "max_bytes": 4096 }
                }
            }"#,
        )
        .unwrap();

        let error = config.validate_agent_settings().unwrap_err();
        assert!(matches!(
            error,
            LoadError::CapabilityUnsupported {
                capability: "state"
            }
        ));
    }

    #[cfg(feature = "state")]
    #[test]
    fn parses_state_settings() {
        let config: Config = serde_json::from_str(
            r#"{
                "agent": {
                    "state": { "max_bytes": 4096 }
                }
            }"#,
        )
        .unwrap();

        assert_eq!(config.state_settings().unwrap().max_bytes, 4096);
    }

    #[cfg(feature = "state")]
    #[test]
    fn defaults_state_settings_when_the_section_is_absent() {
        let config: Config = serde_json::from_str("{}").unwrap();

        assert_eq!(config.state_settings().unwrap().max_bytes, 1_048_576);
    }

    #[cfg(feature = "state")]
    #[test]
    fn names_the_agent_state_section_when_settings_are_invalid() {
        let config: Config = serde_json::from_str(
            r#"{
                "agent": {
                    "state": { "max_bytes": "big" }
                }
            }"#,
        )
        .unwrap();

        let error = config.validate_agent_settings().unwrap_err();
        let LoadError::InvalidAgentConfigSection {
            section: "agent.state",
            source,
        } = error
        else {
            panic!("expected invalid state config");
        };
        assert!(source.to_string().contains("invalid type"));
    }
}
