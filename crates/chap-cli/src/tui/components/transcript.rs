use super::{MessageView, PendingSteeringView, StatusView, ToolView};
use crate::tui::{ChatMessage, model::TranscriptModel};
use chap_core::SteeringId;
use iocraft::prelude::*;

#[derive(Default, Props)]
pub struct TranscriptProps {
    pub model: TranscriptModel,
    pub on_discard: Handler<SteeringId>,
}

#[component]
pub fn Transcript(props: &TranscriptProps) -> impl Into<AnyElement<'static>> {
    let messages = props
        .model
        .messages
        .iter()
        .enumerate()
        .map(|(index, message)| match message {
            ChatMessage::Text { role, content } => element! {
                MessageView(
                    key: index,
                    role: *role,
                    content: content.clone(),
                )
            }
            .into_any(),
            ChatMessage::Tool(tool) => element! {
                ToolView(key: index, tool: tool.clone())
            }
            .into_any(),
            ChatMessage::Status(content) => element! {
                StatusView(key: index, content: content.clone())
            }
            .into_any(),
        })
        .collect::<Vec<_>>();
    let pending = props
        .model
        .pending_steering
        .iter()
        .enumerate()
        .map(|(index, steering)| {
            element! {
                PendingSteeringView(
                    key: props.model.messages.len() + index,
                    steering: Some(steering.clone()),
                    on_discard: props.on_discard.clone(),
                )
            }
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
            ScrollView(auto_scroll: true, keyboard_scroll: Some(false)) {
                View(
                    width: 100pct,
                    flex_direction: FlexDirection::Column,
                    row_gap: 1,
                ) {
                    #(messages)
                    #(pending)
                }
            }
        }
    }
}
