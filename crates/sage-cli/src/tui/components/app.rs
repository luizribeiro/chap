use super::{Footer, Header, Prompt, Transcript};
use crate::tui::{
    ChatMessage, TuiContext,
    model::{TranscriptModel, apply_event},
};
use iocraft::prelude::*;

#[component]
pub fn Sage(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let session = hooks.use_context::<TuiContext>().session.clone();
    let provider = session.provider().to_owned();
    let mut system = hooks.use_context_mut::<SystemContext>();
    let mut input = hooks.use_state(String::new);
    let mut transcript = hooks.use_state(TranscriptModel::default);
    let mut busy = hooks.use_state(|| false);
    let mut should_exit = hooks.use_state(|| false);
    let (terminal_width, terminal_height) = hooks.use_terminal_size();

    let mut events = session.subscribe();
    hooks.use_future(async move {
        loop {
            match events.recv().await {
                Ok(Some(event)) => {
                    let busy_update = apply_event(&mut transcript.write(), event.kind);
                    if let Some(value) = busy_update {
                        busy.set(value);
                    }
                }
                Ok(None) => {
                    busy.set(false);
                    break;
                }
                Err(error) => {
                    transcript
                        .write()
                        .messages
                        .push(ChatMessage::error(error.to_string()));
                    busy.set(false);
                }
            }
        }
    });

    let send = hooks.use_async_handler({
        let session = session.clone();
        move |prompt: String| {
            let session = session.clone();
            async move {
                let _ = session.send(prompt).await;
            }
        }
    });

    hooks.use_terminal_events({
        let session = session.clone();
        move |event| {
            let TerminalEvent::Key(KeyEvent {
                code,
                kind: KeyEventKind::Press,
                modifiers,
                ..
            }) = event
            else {
                return;
            };

            match code {
                KeyCode::Char('d') if modifiers.contains(KeyModifiers::CONTROL) => {
                    should_exit.set(true);
                }
                KeyCode::Esc => {
                    let _ = session.interrupt();
                }
                KeyCode::Enter => {
                    let prompt = input.read().trim().to_owned();
                    if prompt.is_empty() {
                        return;
                    }

                    input.set(String::new());
                    if session.steer(prompt.clone()).is_err() {
                        busy.set(true);
                        send(prompt);
                    }
                }
                _ => {}
            }
        }
    });

    if should_exit.get() {
        system.exit();
    }

    element! {
        View(
            width: terminal_width,
            height: terminal_height,
            flex_direction: FlexDirection::Column,
        ) {
            Header(provider: provider)
            Transcript(
                model: transcript.read().clone(),
                on_discard: {
                    let session = session.clone();
                    move |id| {
                        let _ = session.discard_steering(id);
                    }
                },
            )
            Prompt(
                value: input.to_string(),
                on_change: move |value| input.set(value),
            )
            Footer(busy: busy.get())
        }
    }
}
