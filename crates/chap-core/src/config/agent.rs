use super::{Config, LoadError};
use crate::tool::ExecutionMode;
use lockgate::CallBudget;
use serde::{Deserialize, Deserializer, de};
use serde_json::Value;
use std::{num::NonZeroUsize, time::Duration};

const DEFAULT_PLUGIN_FUEL: u64 = 25_000_000;
const DEFAULT_ADMISSION_DEADLINE_MS: u64 = 30_000;
const DEFAULT_PROVIDER_DEADLINE_MS: u64 = 600_000;
const DEFAULT_HTTP_REQUEST_TIMEOUT_CEILING_MS: u64 = 300_000;
const MAX_HTTP_REQUEST_TIMEOUT_CEILING_MS: u64 = 600_000;
const DEFAULT_TOOLS_DEADLINE_MS: u64 = 30_000;
const DEFAULT_CONTEXT_DEADLINE_MS: u64 = 10_000;
const MAX_PLUGIN_FUEL: u64 = 1_000_000_000;
const MAX_PLUGIN_DEADLINE_MS: u64 = 10 * 60 * 1_000;

/// Settings that control agent-wide behavior.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentSettings {
    /// How tool calls are scheduled across the agent.
    #[serde(default)]
    tool_execution: ToolExecutionSettings,
    #[serde(default)]
    budgets: PluginBudgetOverrides,
    #[serde(default)]
    http: HttpSettings,
    exec: Option<Value>,
    state: Option<Value>,
    vm: Option<Value>,
}

/// Host ceiling for every plugin HTTP request, independent of invocation budgets.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct HttpSettings {
    request_timeout_ceiling_ms: HttpRequestTimeoutCeilingMs,
}

#[derive(Clone, Copy, Debug)]
struct HttpRequestTimeoutCeilingMs(u64);

impl Default for HttpRequestTimeoutCeilingMs {
    fn default() -> Self {
        Self(DEFAULT_HTTP_REQUEST_TIMEOUT_CEILING_MS)
    }
}

impl<'de> Deserialize<'de> for HttpRequestTimeoutCeilingMs {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_bounded_nonzero(
            deserializer,
            "request_timeout_ceiling_ms",
            MAX_HTTP_REQUEST_TIMEOUT_CEILING_MS,
            "600,000 milliseconds (10 minutes)",
        )
        .map(Self)
    }
}

/// Resolved execution budgets shared by every configured plugin.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PluginBudgetSettings {
    pub(crate) admission: CallBudget,
    pub(crate) provider: CallBudget,
    pub(crate) tools: CallBudget,
    pub(crate) context: CallBudget,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PluginBudgetOverrides {
    admission: Option<CallBudgetOverride>,
    provider: Option<CallBudgetOverride>,
    tools: Option<CallBudgetOverride>,
    context: Option<CallBudgetOverride>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct CallBudgetOverride {
    fuel: Option<PluginFuel>,
    deadline_ms: Option<PluginDeadlineMs>,
}

#[derive(Clone, Copy, Debug)]
struct PluginFuel(u64);

#[derive(Clone, Copy, Debug)]
struct PluginDeadlineMs(u64);

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

    pub(crate) fn http_request_timeout_ceiling(&self) -> Duration {
        Duration::from_millis(self.agent.http.request_timeout_ceiling_ms.0)
    }

    pub(crate) fn plugin_budgets(&self) -> PluginBudgetSettings {
        self.agent.budgets.resolve()
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

        if self.agent.vm.is_some() && !cfg!(feature = "vm") {
            return Err(LoadError::CapabilityUnsupported { capability: "vm" });
        }

        #[cfg(feature = "vm")]
        self.vm_settings()?;

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

    #[cfg(feature = "vm")]
    pub(crate) fn vm_settings(&self) -> Result<chap_vm::host::VmSettings, LoadError> {
        self.agent
            .vm
            .clone()
            .map(serde_json::from_value)
            .transpose()
            .map(Option::unwrap_or_default)
            .map_err(|source| LoadError::InvalidAgentConfigSection {
                section: "agent.vm",
                source,
            })
    }
}

impl Default for PluginBudgetSettings {
    fn default() -> Self {
        Self {
            admission: default_plugin_budget(DEFAULT_ADMISSION_DEADLINE_MS),
            provider: default_plugin_budget(DEFAULT_PROVIDER_DEADLINE_MS),
            tools: default_plugin_budget(DEFAULT_TOOLS_DEADLINE_MS),
            context: default_plugin_budget(DEFAULT_CONTEXT_DEADLINE_MS),
        }
    }
}

impl PluginBudgetOverrides {
    fn resolve(self) -> PluginBudgetSettings {
        let defaults = PluginBudgetSettings::default();
        PluginBudgetSettings {
            admission: self.admission.resolve(defaults.admission),
            provider: self.provider.resolve(defaults.provider),
            tools: self.tools.resolve(defaults.tools),
            context: self.context.resolve(defaults.context),
        }
    }
}

trait ResolveCallBudget {
    fn resolve(self, default: CallBudget) -> CallBudget;
}

impl ResolveCallBudget for Option<CallBudgetOverride> {
    fn resolve(self, default: CallBudget) -> CallBudget {
        let Some(settings) = self else {
            return default;
        };
        CallBudget {
            fuel: settings.fuel.map_or(default.fuel, |fuel| fuel.0),
            deadline: settings.deadline_ms.map_or(default.deadline, |deadline| {
                Duration::from_millis(deadline.0)
            }),
        }
    }
}

impl<'de> Deserialize<'de> for PluginFuel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_bounded_nonzero(
            deserializer,
            "fuel",
            MAX_PLUGIN_FUEL,
            "1,000,000,000 instructions",
        )
        .map(Self)
    }
}

impl<'de> Deserialize<'de> for PluginDeadlineMs {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_bounded_nonzero(
            deserializer,
            "deadline_ms",
            MAX_PLUGIN_DEADLINE_MS,
            "600,000 milliseconds (10 minutes)",
        )
        .map(Self)
    }
}

fn deserialize_bounded_nonzero<'de, D>(
    deserializer: D,
    field: &str,
    maximum: u64,
    maximum_description: &str,
) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if value == 0 {
        return Err(de::Error::custom(format_args!(
            "`{field}` must be greater than zero"
        )));
    }
    if value > maximum {
        return Err(de::Error::custom(format_args!(
            "`{field}` must not exceed {maximum_description}"
        )));
    }
    Ok(value)
}

fn default_plugin_budget(deadline_ms: u64) -> CallBudget {
    CallBudget {
        fuel: DEFAULT_PLUGIN_FUEL,
        deadline: Duration::from_millis(deadline_ms),
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
    fn defaults_http_ceiling_when_sections_or_field_are_absent() {
        for source in ["{}", r#"{"agent":{}}"#, r#"{"agent":{"http":{}}}"#] {
            let config = load_config(source).unwrap();
            assert_eq!(
                config.http_request_timeout_ceiling(),
                Duration::from_secs(300)
            );
            assert_eq!(
                config.plugin_budgets().provider.deadline,
                Duration::from_secs(600)
            );
        }
    }

    #[test]
    fn accepts_http_ceiling_bounds_and_keeps_provider_budget_separate() {
        for value in [1, 120_000, 360_000, 600_000] {
            let config = load_config(&format!(
                r#"{{"agent":{{"http":{{"request_timeout_ceiling_ms":{value}}},"budgets":{{"provider":{{"deadline_ms":390000}}}}}}}}"#
            )).unwrap();
            assert_eq!(
                config.http_request_timeout_ceiling(),
                Duration::from_millis(value)
            );
            assert_eq!(
                config.plugin_budgets().provider.deadline,
                Duration::from_secs(390)
            );
        }
    }

    #[test]
    fn rejects_invalid_http_ceilings_and_unknown_fields() {
        for (http, expected) in [
            (
                r#"{"request_timeout_ceiling_ms":0}"#,
                "must be greater than zero",
            ),
            (
                r#"{"request_timeout_ceiling_ms":600001}"#,
                "must not exceed 600,000",
            ),
            (r#"{"request_timeout_ceiling_ms":-1}"#, "invalid value"),
            (
                r#"{"request_timeout_ceiling_ms":18446744073709551616}"#,
                "invalid type",
            ),
            (r#"{"request_timeout_ceiling_ms":1.5}"#, "invalid type"),
            (r#"{"request_timeout_ceiling_ms":"120000"}"#, "invalid type"),
            (r#"{"request_timeout_ceiling_ms":null}"#, "invalid type"),
            (r#"{"timeout_ms":120000}"#, "unknown field `timeout_ms`"),
        ] {
            let error = load_config(&format!(r#"{{"agent":{{"http":{http}}}}}"#)).unwrap_err();
            let LoadError::ParseConfig { source, .. } = error else {
                panic!("expected config parse failure");
            };
            assert!(source.to_string().contains(expected), "{source}");
        }
    }

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
    fn parses_plugin_budget_overrides_and_preserves_unspecified_defaults() {
        let config = load_config(
            r#"{
                "agent": {
                    "budgets": {
                        "admission": { "fuel": 100, "deadline_ms": 101 },
                        "provider": { "fuel": 200 },
                        "tools": { "deadline_ms": 300 },
                        "context": { "fuel": 1000000000, "deadline_ms": 600000 }
                    }
                }
            }"#,
        )
        .unwrap();

        let budgets = config.plugin_budgets();
        assert_eq!(
            budgets.admission,
            CallBudget {
                fuel: 100,
                deadline: Duration::from_millis(101),
            }
        );
        assert_eq!(
            budgets.provider,
            CallBudget {
                fuel: 200,
                deadline: Duration::from_millis(DEFAULT_PROVIDER_DEADLINE_MS),
            }
        );
        assert_eq!(
            budgets.tools,
            CallBudget {
                fuel: DEFAULT_PLUGIN_FUEL,
                deadline: Duration::from_millis(300),
            }
        );
        assert_eq!(
            budgets.context,
            CallBudget {
                fuel: MAX_PLUGIN_FUEL,
                deadline: Duration::from_millis(MAX_PLUGIN_DEADLINE_MS),
            }
        );
    }

    #[test]
    fn rejects_zero_and_over_ceiling_plugin_budgets() {
        for (field, value, expected) in [
            ("fuel", 0, "`fuel` must be greater than zero"),
            (
                "fuel",
                MAX_PLUGIN_FUEL + 1,
                "`fuel` must not exceed 1,000,000,000 instructions",
            ),
            ("deadline_ms", 0, "`deadline_ms` must be greater than zero"),
            (
                "deadline_ms",
                MAX_PLUGIN_DEADLINE_MS + 1,
                "`deadline_ms` must not exceed 600,000 milliseconds (10 minutes)",
            ),
        ] {
            let error = load_config(&format!(
                r#"{{
                    "agent": {{
                        "budgets": {{
                            "provider": {{ "{field}": {value} }}
                        }}
                    }}
                }}"#
            ))
            .unwrap_err();

            let LoadError::ParseConfig { source, .. } = error else {
                panic!("expected config parse failure");
            };
            assert!(source.to_string().contains(expected), "{source}");
        }
    }

    #[test]
    fn rejects_unknown_plugin_budget_fields() {
        for source in [
            r#"{ "agent": { "budgets": { "tool": {} } } }"#,
            r#"{ "agent": { "budgets": { "tools": { "timeout_ms": 1000 } } } }"#,
        ] {
            let error = load_config(source).unwrap_err();
            let LoadError::ParseConfig { source, .. } = error else {
                panic!("expected config parse failure");
            };
            assert!(source.to_string().contains("unknown field"), "{source}");
        }
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
                        "timeout_ceiling_ms": 1000,
                        "max_concurrent_processes": 2
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
        assert_eq!(exec.max_concurrent_processes.get(), 2);
    }

    #[cfg(feature = "exec")]
    #[test]
    fn defaults_exec_settings_when_the_section_is_absent() {
        let config: Config = serde_json::from_str("{}").unwrap();
        let exec = config.exec_settings().unwrap();

        assert!(exec.path.is_none());
        assert_eq!(exec.timeout_ceiling_ms, 120_000);
        assert_eq!(exec.max_concurrent_processes.get(), 4);
    }

    #[cfg(feature = "exec")]
    #[test]
    fn rejects_a_zero_exec_process_limit() {
        let config: Config = serde_json::from_str(
            r#"{
                "agent": {
                    "exec": { "max_concurrent_processes": 0 }
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
        assert!(source.to_string().contains("nonzero"));
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

    #[cfg(not(feature = "vm"))]
    #[test]
    fn rejects_vm_settings_when_the_capability_is_unavailable() {
        let config: Config = serde_json::from_str(
            r#"{
                "agent": {
                    "vm": { "limits": { "max_vms_per_plugin": 3 } }
                }
            }"#,
        )
        .unwrap();

        let error = config.validate_agent_settings().unwrap_err();
        assert!(matches!(
            error,
            LoadError::CapabilityUnsupported { capability: "vm" }
        ));
    }

    #[cfg(feature = "vm")]
    #[test]
    fn parses_vm_settings() {
        let config: Config = serde_json::from_str(
            r#"{
                "agent": {
                    "vm": {
                        "registries": ["ghcr.io"],
                        "limits": { "max_vms_per_plugin": 3 },
                        "instance": {
                            "cpus": 2,
                            "memory_mb": 1024,
                            "max_lifetime_ms": 7200000,
                            "idle_timeout_ms": 600000
                        },
                        "calls": {
                            "create_timeout_ms": 180000,
                            "destroy_timeout_ms": 45000,
                            "exec_timeout_ceiling_ms": 30000,
                            "exec_max_output_bytes": 32768,
                            "read_file_max_bytes": 1048576
                        }
                    }
                }
            }"#,
        )
        .unwrap();

        let vm = config.vm_settings().unwrap();
        assert_eq!(vm.registries, ["ghcr.io"]);
        assert_eq!(vm.limits.max_vms_per_plugin, 3);
        assert_eq!(vm.instance.cpus, 2);
        assert_eq!(vm.instance.memory_mb, 1024);
        assert_eq!(vm.instance.max_lifetime_ms, 7_200_000);
        assert_eq!(vm.instance.idle_timeout_ms, 600_000);
        assert_eq!(vm.calls.create_timeout_ms, 180_000);
        assert_eq!(vm.calls.destroy_timeout_ms, 45_000);
        assert_eq!(vm.calls.exec_timeout_ceiling_ms, 30_000);
        assert_eq!(vm.calls.exec_max_output_bytes, 32_768);
        assert_eq!(vm.calls.read_file_max_bytes, 1_048_576);
    }

    #[cfg(feature = "vm")]
    #[test]
    fn defaults_vm_settings_when_the_section_is_absent() {
        let config: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(
            config.vm_settings().unwrap(),
            chap_vm::host::VmSettings::default()
        );
    }

    #[cfg(feature = "vm")]
    #[test]
    fn names_the_agent_vm_section_when_settings_are_invalid() {
        let config: Config = serde_json::from_str(
            r#"{
                "agent": {
                    "vm": { "limits": { "max_vms_per_plugin": "many" } }
                }
            }"#,
        )
        .unwrap();

        let error = config.validate_agent_settings().unwrap_err();
        let LoadError::InvalidAgentConfigSection {
            section: "agent.vm",
            source,
        } = error
        else {
            panic!("expected invalid vm config");
        };
        assert!(source.to_string().contains("invalid type"));
    }
}
