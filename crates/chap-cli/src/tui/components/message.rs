use crate::tui::model::MessageRole;
use iocraft::prelude::*;
use std::sync::Arc;

use super::Markdown;

#[derive(Default, Props)]
pub struct MessageViewProps {
    pub role: MessageRole,
    pub content: Arc<str>,
}

#[component]
pub fn MessageView(props: &MessageViewProps) -> impl Into<AnyElement<'static>> {
    let (label, color) = match props.role {
        MessageRole::User => ("you", Color::Blue),
        MessageRole::Chap => ("chap", Color::Green),
        MessageRole::Error => ("error", Color::Red),
    };

    match props.role {
        MessageRole::Chap => element! {
            View(flex_direction: FlexDirection::Column) {
                Text(content: label, color, weight: Weight::Bold)
                Markdown(content: Arc::clone(&props.content))
            }
        }
        .into_any(),
        MessageRole::User | MessageRole::Error => element! {
            View(flex_direction: FlexDirection::Column) {
                Text(content: label, color, weight: Weight::Bold)
                Text(content: props.content.to_string())
            }
        }
        .into_any(),
    }
}
