mod components;
mod diagnostics;
mod editor;
mod model;

use chap_core::{Session, SessionOptions, WorkspaceInfo};
use components::Chap;
use diagnostics::TuiDiagnostics;
use iocraft::prelude::*;
use model::ChatMessage;
use std::{fmt, io::IsTerminal, path::Path};
use tokio::task::JoinHandle;

pub(crate) use diagnostics::DiagnosticWriter;

const PROVIDER: &str = "openai";

struct TuiContext {
    session: Session,
    workspace_status: Option<String>,
}

pub async fn run(
    agent: chap_core::Agent,
    consent_path: &Path,
    tracing: &crate::telemetry::TracingRouter,
) -> Result<(), String> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err("the terminal interface requires an interactive terminal".into());
    }

    let workspace_status = agent.workspace().as_ref().map(format_workspace_status);
    let provider = PROVIDER
        .parse::<chap_core::PluginId>()
        .map_err(|error| error.to_string())?;
    let session = agent
        .session(SessionOptions::new(provider))
        .await
        .map_err(crate::render_session_error)?;
    let session_id = session.id();
    let mut element = element! {
        ContextProvider(value: Context::owned(TuiContext { session, workspace_status })) {
            Chap
        }
    };
    let diagnostics = TuiDiagnostics::install(consent_path, session_id, tracing)?;
    let diagnostic_writer = diagnostics.writer();
    let render_task =
        tokio::spawn(async move { element.fullscreen().stderr(diagnostic_writer).await });
    let result = await_terminal_task(render_task, |error| {
        format!("terminal interface failed: {error}")
    })
    .await;
    diagnostics.finish();
    result
}

fn format_workspace_status(workspace: &WorkspaceInfo) -> String {
    let access = if workspace.readonly {
        "read-only"
    } else {
        "read-write"
    };
    let mut status = format!(
        "workspace: {} ({access}) in {}",
        workspace.directory.display(),
        workspace.image
    );
    if !workspace.secrets.is_empty() {
        status.push_str(", secrets: ");
        status.push_str(
            &workspace
                .secrets
                .iter()
                .map(|secret| format!("{} → {}", secret.env, secret.hosts.join(", ")))
                .collect::<Vec<_>>()
                .join("; "),
        );
    }
    status
}

async fn await_terminal_task<E>(
    task: JoinHandle<Result<(), E>>,
    format_error: impl FnOnce(String) -> String,
) -> Result<(), String>
where
    E: fmt::Display + Send + 'static,
{
    let error = match task.await {
        Ok(Ok(())) => return Ok(()),
        Ok(Err(error)) => error.to_string(),
        Err(error) => error.to_string(),
    };
    Err(format_error(error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    struct RestoreOnDrop(Arc<AtomicBool>);

    #[test]
    fn formats_the_workspace_startup_status() {
        let workspace = WorkspaceInfo {
            directory: "/project".into(),
            readonly: true,
            image: "docker.io/library/alpine:3.20".into(),
            egress: vec![],
            secrets: vec![],
            exec_timeout_ms: 30_000,
        };

        assert_eq!(
            format_workspace_status(&workspace),
            "workspace: /project (read-only) in docker.io/library/alpine:3.20"
        );

        let with_secrets = WorkspaceInfo {
            secrets: vec![chap_core::WorkspaceSecret {
                env: "GITHUB_TOKEN".into(),
                hosts: vec!["api.github.com".into(), "github.com".into()],
            }],
            ..workspace
        };
        assert_eq!(
            format_workspace_status(&with_secrets),
            "workspace: /project (read-only) in docker.io/library/alpine:3.20, secrets: GITHUB_TOKEN → api.github.com, github.com"
        );
    }

    impl Drop for RestoreOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn terminal_is_restored_before_an_error_is_formatted() {
        let restored = Arc::new(AtomicBool::new(false));
        let task_restored = Arc::clone(&restored);
        let render_task = tokio::spawn(async move {
            let _guard = RestoreOnDrop(task_restored);
            Err::<(), _>("render failed")
        });
        let formatter_restored = Arc::clone(&restored);

        let error = await_terminal_task(render_task, move |error| {
            assert!(formatter_restored.load(Ordering::SeqCst));
            format!("terminal interface failed: {error}")
        })
        .await
        .unwrap_err();

        assert_eq!(error, "terminal interface failed: render failed");
    }

    #[tokio::test]
    async fn terminal_is_restored_before_a_panic_is_formatted() {
        let restored = Arc::new(AtomicBool::new(false));
        let task_restored = Arc::clone(&restored);
        let render_task: JoinHandle<Result<(), String>> = tokio::spawn(async move {
            let _guard = RestoreOnDrop(task_restored);
            panic!("render panicked")
        });
        let formatter_restored = Arc::clone(&restored);

        let error = await_terminal_task(render_task, move |error| {
            assert!(formatter_restored.load(Ordering::SeqCst));
            format!("terminal interface failed: {error}")
        })
        .await
        .unwrap_err();

        assert!(error.starts_with("terminal interface failed: task "));
        assert!(error.contains("panicked with message \"render panicked\""));
    }
}
