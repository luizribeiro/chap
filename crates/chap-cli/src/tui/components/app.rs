use super::{Footer, Header, Prompt, Transcript};
use crate::tui::{
    ChatMessage, TuiContext,
    model::{TranscriptModel, apply_event},
};
use iocraft::prelude::*;

#[component]
pub fn Chap(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let session = hooks.use_context::<TuiContext>().session.clone();
    let provider = session.provider().to_owned();
    let mut system = hooks.use_context_mut::<SystemContext>();
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
                    session.interrupt();
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
                        if let Err(error) = session.discard_steering(id) {
                            let mut transcript = transcript;
                            transcript
                                .write()
                                .messages
                                .push(ChatMessage::error(error.to_string()));
                        }
                    }
                },
            )
            Prompt(
                on_submit: {
                    let session = session.clone();
                    let send = send.clone();
                    move |prompt: String| {
                        if session.steer(prompt.clone()).is_err() {
                            let mut busy = busy;
                            busy.set(true);
                            send(prompt);
                        }
                    }
                },
                on_error: move |error| {
                    let mut transcript = transcript;
                    transcript.write().messages.push(ChatMessage::error(error));
                },
            )
            Footer(busy: busy.get(), usage: transcript.read().usage)
        }
    }
}
