use super::{Footer, Header, Prompt, Transcript};
use crate::tui::{ChatMessage, TuiContext, model::apply_event};
use iocraft::prelude::*;

#[component]
pub fn Sage(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let session = hooks.use_context::<TuiContext>().session.clone();
    let provider = session.provider().to_owned();
    let mut system = hooks.use_context_mut::<SystemContext>();
    let mut input = hooks.use_state(String::new);
    let mut messages = hooks.use_state(Vec::<ChatMessage>::new);
    let mut busy = hooks.use_state(|| false);
    let mut should_exit = hooks.use_state(|| false);
    let (terminal_width, terminal_height) = hooks.use_terminal_size();

    let mut events = session.subscribe();
    hooks.use_future(async move {
        loop {
            match events.recv().await {
                Ok(Some(event)) => {
                    let busy_update = {
                        let mut messages = messages.write();
                        apply_event(&mut messages, event.kind)
                    };
                    if let Some(value) = busy_update {
                        busy.set(value);
                    }
                }
                Ok(None) => {
                    busy.set(false);
                    break;
                }
                Err(error) => {
                    messages.write().push(ChatMessage::error(error.to_string()));
                    busy.set(false);
                }
            }
        }
    });

    let complete = hooks.use_async_handler({
        move |prompt: String| {
            let session = session.clone();
            async move {
                let _ = session.send(prompt).await;
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
