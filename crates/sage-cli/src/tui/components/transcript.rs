use super::MessageView;
use crate::tui::ChatMessage;
use iocraft::prelude::*;

#[derive(Default, Props)]
pub struct TranscriptProps {
    pub messages: Vec<ChatMessage>,
}

#[component]
pub fn Transcript(props: &TranscriptProps) -> impl Into<AnyElement<'static>> {
    let messages = props
        .messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            element! {
                MessageView(
                    key: index,
                    role: message.role,
                    content: message.content.clone(),
                )
            }
            .into_any()
        })
        .collect::<Vec<_>>();

    element! {
        View(
            width: 100pct,
            height: 0,
            flex_grow: 1.0_f32,
            padding_left: 1,
            padding_right: 1,
            padding_top: 1,
            margin_bottom: 1,
        ) {
            ScrollView(auto_scroll: true) {
                View(
                    width: 100pct,
                    flex_direction: FlexDirection::Column,
                    row_gap: 1,
                ) {
                    #(messages)
                }
            }
        }
    }
}
