mod env;
mod resolve;
mod resource;
mod sandbox;
mod spawn;

use std::collections::HashMap;
use std::fmt;
use std::format;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::string::String;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::vec::Vec;

use serde::Deserialize;
use tokio::process::Command;
use tokio::sync::Semaphore;

pub use resource::CommandTarget;

const DEFAULT_TIMEOUT_CEILING_MS: u64 = 120_000;

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExecSettings {
    pub path: Option<Vec<PathBuf>>,
    pub timeout_ceiling_ms: u64,
    pub max_concurrent_processes: NonZeroUsize,
}

impl Default for ExecSettings {
    fn default() -> Self {
        Self {
            path: None,
            timeout_ceiling_ms: DEFAULT_TIMEOUT_CEILING_MS,
            max_concurrent_processes: NonZeroUsize::new(4)
                .expect("the default process limit is nonzero"),
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
    max_concurrent_processes: usize,
    spawn_slots: Mutex<HashMap<String, Arc<Semaphore>>>,
}

impl Executor {
    pub fn new(settings: ExecSettings, project_root: impl Into<PathBuf>) -> Self {
        let project_root = project_root.into();
        Self {
            path: resolve::pinned_search_path(settings.path),
            timeout_ceiling: Duration::from_millis(settings.timeout_ceiling_ms),
            project_root: std::path::absolute(&project_root).unwrap_or(project_root),
            sandbox: sandbox::Sandbox::None,
            max_concurrent_processes: settings.max_concurrent_processes.get(),
            spawn_slots: Mutex::new(HashMap::new()),
        }
    }

    fn spawn_slots_for(&self, plugin_id: &str) -> Arc<Semaphore> {
        let mut slots = self
            .spawn_slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Arc::clone(
            slots
                .entry(String::from(plugin_id))
                .or_insert_with(|| Arc::new(Semaphore::new(self.max_concurrent_processes))),
        )
    }

    pub async fn execute(
        &self,
        plugin_id: &str,
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
        let _permit = self
            .spawn_slots_for(plugin_id)
            .acquire_owned()
            .await
            .map_err(|error| {
                ExecError::Failed(format!("could not reserve an exec process slot: {error}"))
            })?;

        spawn::run(
            command,
            &target.program,
            &self.project_root,
            effective_timeout,
        )
        .await
    }
}

#[cfg(test)]
mod host_tests {
    use std::fs;
    use std::num::NonZeroUsize;
    use std::string::String;
    use std::sync::Arc;
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
            .execute("plugin", &target("echo", &["executor-ok"]), None)
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
                "plugin",
                &target("sh", &["-c", "sleep 30"]),
                Some(Duration::from_secs(10)),
            )
            .await
            .unwrap_err();

        assert_eq!(error, ExecError::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn executor_serializes_spawns_at_the_configured_limit() {
        let project = TempDir::new().unwrap();
        let executor = Arc::new(Executor::new(
            ExecSettings {
                max_concurrent_processes: NonZeroUsize::new(1).unwrap(),
                ..ExecSettings::default()
            },
            project.path(),
        ));

        let first_executor = Arc::clone(&executor);
        let first = tokio::spawn(async move {
            first_executor
                .execute(
                    "plugin",
                    &target(
                        "sh",
                        &[
                            "-c",
                            "touch first-started; sleep 0.3; test ! -e second-started; touch first-finished",
                        ],
                    ),
                    None,
                )
                .await
        });

        let first_started = project.path().join("first-started");
        tokio::time::timeout(Duration::from_secs(2), async {
            while !first_started.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("first process did not start");

        let second_target = target(
            "sh",
            &["-c", "touch second-started; test -e first-finished"],
        );
        let second = executor.execute("plugin", &second_target, None);
        let (first, second) = tokio::join!(first, second);

        assert_eq!(first.unwrap().unwrap().exit_code, 0);
        assert_eq!(second.unwrap().exit_code, 0);
        assert!(fs::exists(project.path().join("first-finished")).unwrap());
        assert!(fs::exists(project.path().join("second-started")).unwrap());
    }

    #[tokio::test]
    async fn spawn_limit_is_counted_per_plugin() {
        let project = TempDir::new().unwrap();
        let executor = Arc::new(Executor::new(
            ExecSettings {
                max_concurrent_processes: NonZeroUsize::new(1).unwrap(),
                ..ExecSettings::default()
            },
            project.path(),
        ));

        let first_executor = Arc::clone(&executor);
        let first = tokio::spawn(async move {
            first_executor
                .execute(
                    "holder",
                    &target(
                        "sh",
                        &[
                            "-c",
                            "touch holder-started; sleep 0.3; touch holder-finished",
                        ],
                    ),
                    None,
                )
                .await
        });

        let holder_started = project.path().join("holder-started");
        tokio::time::timeout(Duration::from_secs(2), async {
            while !holder_started.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("holder process did not start");

        let other = executor
            .execute(
                "other",
                &target("sh", &["-c", "test ! -e holder-finished"]),
                None,
            )
            .await
            .unwrap();

        assert_eq!(
            other.exit_code, 0,
            "the other plugin waited for the holder's slot"
        );
        assert_eq!(first.await.unwrap().unwrap().exit_code, 0);
    }
}
