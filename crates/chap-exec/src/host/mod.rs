mod env;
mod resolve;
mod resource;
mod sandbox;
mod spawn;

use std::fmt;
use std::path::PathBuf;
use std::string::String;
use std::time::Duration;
use std::vec::Vec;

use serde::Deserialize;
use tokio::process::Command;

pub use resource::CommandTarget;

const DEFAULT_TIMEOUT_CEILING_MS: u64 = 120_000;

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExecSettings {
    pub path: Option<Vec<PathBuf>>,
    pub timeout_ceiling_ms: u64,
}

impl Default for ExecSettings {
    fn default() -> Self {
        Self {
            path: None,
            timeout_ceiling_ms: DEFAULT_TIMEOUT_CEILING_MS,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecOutcome {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecError {
    Rejected(String),
    TimedOut,
    Failed(String),
}

impl fmt::Display for ExecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(message) | Self::Failed(message) => formatter.write_str(message),
            Self::TimedOut => formatter.write_str("command timed out"),
        }
    }
}

impl std::error::Error for ExecError {}

pub struct Executor {
    path: Vec<PathBuf>,
    timeout_ceiling: Duration,
    project_root: PathBuf,
    sandbox: sandbox::Sandbox,
}

impl Executor {
    pub fn new(settings: ExecSettings, project_root: impl Into<PathBuf>) -> Self {
        let project_root = project_root.into();
        Self {
            path: resolve::pinned_search_path(settings.path),
            timeout_ceiling: Duration::from_millis(settings.timeout_ceiling_ms),
            project_root: std::path::absolute(&project_root).unwrap_or(project_root),
            sandbox: sandbox::Sandbox::None,
        }
    }

    pub async fn execute(
        &self,
        target: &CommandTarget,
        timeout: Option<Duration>,
    ) -> Result<ExecOutcome, ExecError> {
        let program = resolve::program(&target.program, &self.path)?;
        let environment = env::constructed(&self.path)?;

        let mut command = Command::new(program);
        command.args(&target.args).env_clear().envs(environment);
        let command = self.sandbox.wrap(command);
        let effective_timeout = timeout
            .unwrap_or(self.timeout_ceiling)
            .min(self.timeout_ceiling);

        spawn::run(command, &self.project_root, effective_timeout).await
    }
}

#[cfg(test)]
mod host_tests {
    use std::string::String;
    use std::time::{Duration, Instant};

    use tempfile::TempDir;

    use super::{CommandTarget, ExecError, ExecSettings, Executor};

    fn target(program: &str, args: &[&str]) -> CommandTarget {
        CommandTarget {
            program: String::from(program),
            args: args.iter().map(|arg| String::from(*arg)).collect(),
        }
    }

    #[tokio::test]
    async fn executor_runs_a_real_command_end_to_end() {
        let project = TempDir::new().unwrap();
        let executor = Executor::new(ExecSettings::default(), project.path());

        let outcome = executor
            .execute(&target("echo", &["executor-ok"]), None)
            .await
            .unwrap();

        assert_eq!(outcome.exit_code, 0, "stderr: {:?}", outcome.stderr);
        assert_eq!(outcome.stdout, "executor-ok\n");
        assert_eq!(outcome.stderr, "");
        assert!(!outcome.truncated);
    }

    #[tokio::test]
    async fn executor_clamps_requested_timeouts_to_its_ceiling() {
        let project = TempDir::new().unwrap();
        let executor = Executor::new(
            ExecSettings {
                timeout_ceiling_ms: 100,
                ..ExecSettings::default()
            },
            project.path(),
        );
        let started = Instant::now();

        let error = executor
            .execute(
                &target("sh", &["-c", "sleep 30"]),
                Some(Duration::from_secs(10)),
            )
            .await
            .unwrap_err();

        assert_eq!(error, ExecError::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
