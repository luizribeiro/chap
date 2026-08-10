use crate::tui::editor;
use iocraft::prelude::*;

#[derive(Default, Props)]
pub struct PromptProps {
    pub on_submit: Handler<String>,
    pub on_error: Handler<String>,
}

#[component]
pub fn Prompt(props: &PromptProps, mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let mut system = hooks.use_context_mut::<SystemContext>();
    let mut input = hooks.use_state(String::new);
    let mut terminal_has_focus = hooks.use_state(|| true);
    let mut editor_requested = hooks.use_state(|| false);
    let on_submit = props.on_submit.clone();

    hooks.use_terminal_events(move |event| match event {
        TerminalEvent::FocusGained => terminal_has_focus.set(true),
        TerminalEvent::FocusLost => terminal_has_focus.set(false),
        TerminalEvent::Key(KeyEvent {
            code,
            kind: KeyEventKind::Press,
            modifiers,
            ..
        }) => match code {
            KeyCode::Char('g') if modifiers.contains(KeyModifiers::CONTROL) => {
                editor_requested.set(true);
            }
            KeyCode::Enter => {
                let prompt = input.read().trim().to_owned();
                if prompt.is_empty() {
                    return;
                }

                input.set(String::new());
                on_submit(prompt);
            }
            _ => {}
        },
        _ => {}
    });

    if editor_requested.get() {
        editor_requested.set(false);
        let draft = input.to_string();
        let on_error = props.on_error.clone();
        system.suspend_terminal(move || match editor::edit(&draft) {
            Ok(edited) => input.set(edited),
            Err(error) => on_error(error),
        });
    }

    element! {
        View(
            width: 100pct,
            height: 3,
            padding_left: 1,
            padding_right: 1,
            border_style: BorderStyle::Single,
            border_edges: Edges::Top | Edges::Bottom,
            border_color: Color::DarkGrey,
            flex_direction: FlexDirection::Row,
        ) {
            Text(content: "› ", color: Color::Cyan, weight: Weight::Bold)
            View(flex_grow: 1.0_f32) {
                TextInput(
                    has_focus: terminal_has_focus.get(),
                    value: input.to_string(),
                    on_change: move |value| input.set(value),
                )
            }
        }
    }
}
