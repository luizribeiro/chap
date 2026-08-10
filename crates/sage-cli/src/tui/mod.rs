mod components;
mod model;

use components::Sage;
use iocraft::prelude::*;
use model::ChatMessage;
use sage_core::{Application, Runtime};
use std::{io::IsTerminal, sync::Arc};

const PROVIDER: &str = "openai";

struct TuiContext {
    runtime: Arc<Runtime>,
    provider: String,
}

pub async fn run(application: Application) -> Result<(), String> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err("the terminal interface requires an interactive terminal".into());
    }

    let runtime = Arc::new(application.run().await?);
    let mut element = element! {
        ContextProvider(value: Context::owned(TuiContext {
            runtime,
            provider: PROVIDER.to_owned(),
        })) {
            Sage
        }
    };
    element
        .fullscreen()
        .disable_mouse_capture()
        .await
        .map_err(|error| format!("terminal interface failed: {error}"))
}
