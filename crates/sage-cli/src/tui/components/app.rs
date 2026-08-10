use super::{Footer, Header, Prompt, Transcript};
use crate::tui::{ChatMessage, TuiContext};
use iocraft::prelude::*;
use std::sync::Arc;

#[component]
pub fn Sage(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let (runtime, session) = {
        let context = hooks.use_context::<TuiContext>();
        (Arc::clone(&context.runtime), context.session.clone())
    };
    let provider = session.provider().to_owned();
    let mut system = hooks.use_context_mut::<SystemContext>();
    let mut input = hooks.use_state(String::new);
    let mut messages = hooks.use_state(Vec::<ChatMessage>::new);
    let mut busy = hooks.use_state(|| false);
    let mut should_exit = hooks.use_state(|| false);
    let (terminal_width, terminal_height) = hooks.use_terminal_size();

    let complete = hooks.use_async_handler({
        move |prompt: String| {
            let runtime = Arc::clone(&runtime);
            let session = session.clone();
            async move {
                let message = match runtime.run_turn(&session, prompt).await {
                    Ok(completion) => ChatMessage::sage(completion),
                    Err(error) => ChatMessage::error(error),
                };
                messages.write().push(message);
                busy.set(false);
            }
        }
    });

    hooks.use_terminal_events(move |event| {
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
            KeyCode::Enter if !busy.get() => {
                let prompt = input.read().trim().to_owned();
                if prompt.is_empty() {
                    return;
                }

                messages.write().push(ChatMessage::user(prompt.clone()));
                input.set(String::new());
                busy.set(true);
                complete(prompt);
            }
            _ => {}
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
            Transcript(messages: messages.read().clone())
            Prompt(
                busy: busy.get(),
                value: input.to_string(),
                on_change: move |value| input.set(value),
            )
            Footer(busy: busy.get())
        }
    }
}
