mod components;
mod diagnostics;
mod editor;
mod model;

use chap_core::{Session, SessionOptions};
use components::Chap;
use diagnostics::TuiDiagnostics;
use iocraft::prelude::*;
use model::ChatMessage;
use std::{io::IsTerminal, path::Path};

const PROVIDER: &str = "openai";

struct TuiContext {
    session: Session,
}

pub async fn run(agent: chap_core::Agent, consent_path: &Path) -> Result<(), String> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err("the terminal interface requires an interactive terminal".into());
    }

    let session = agent
        .session(SessionOptions::new(PROVIDER))
        .await
        .map_err(crate::render_session_error)?;
    let session_id = session.id();
    let mut element = element! {
        ContextProvider(value: Context::owned(TuiContext { session })) {
            Chap
        }
    };
    let diagnostics = TuiDiagnostics::install(consent_path, session_id)?;
    let result = element
        .fullscreen()
        .stderr(diagnostics.writer())
        .await
        .map_err(|error| format!("terminal interface failed: {error}"));
    diagnostics.finish();
    result
}
