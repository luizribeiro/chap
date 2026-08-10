use crate::tui::model::MessageRole;
use iocraft::prelude::*;

#[derive(Default, Props)]
pub struct MessageViewProps {
    pub role: MessageRole,
    pub content: String,
}

#[component]
pub fn MessageView(props: &MessageViewProps) -> impl Into<AnyElement<'static>> {
    let (label, color) = match props.role {
        MessageRole::User => ("you", Color::Blue),
        MessageRole::Sage => ("sage", Color::Green),
        MessageRole::Error => ("error", Color::Red),
    };

    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: label, color, weight: Weight::Bold)
            Text(content: props.content.clone())
        }
    }
}
