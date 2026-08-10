mod components;
mod editor;
mod model;

use components::Sage;
use iocraft::prelude::*;
use model::ChatMessage;
use sage_core::{Session, SessionOptions};
use std::io::IsTerminal;

const PROVIDER: &str = "openai";

struct TuiContext {
    session: Session,
}

pub async fn run(sage: sage_core::Agent) -> Result<(), String> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err("the terminal interface requires an interactive terminal".into());
    }

    let session = sage.session(SessionOptions::new(PROVIDER))?;
    let mut element = element! {
        ContextProvider(value: Context::owned(TuiContext { session })) {
            Sage
        }
    };
    element
        .fullscreen()
        .await
        .map_err(|error| format!("terminal interface failed: {error}"))
}
