mod components;
mod diagnostics;
mod editor;
mod model;

use chap_core::{Session, SessionOptions};
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
}

pub async fn run(
    agent: chap_core::Agent,
    consent_path: &Path,
    tracing: &crate::telemetry::TracingRouter,
) -> Result<(), String> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err("the terminal interface requires an interactive terminal".into());
    }

    let session = agent
        .session(SessionOptions::new(PROVIDER.into()))
        .await
        .map_err(crate::render_session_error)?;
    let session_id = session.id();
    let mut element = element! {
        ContextProvider(value: Context::owned(TuiContext { session })) {
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
