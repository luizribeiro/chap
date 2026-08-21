use chap_core::{SessionEventKind, SteeringId, Usage};
use std::sync::Arc;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TranscriptModel {
    pub messages: Vec<ChatMessage>,
    pub pending_steering: Vec<PendingSteering>,
    pub usage: Option<UsageModel>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UsageModel {
    pub context_tokens: u64,
    pub session: Usage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingSteering {
    pub id: SteeringId,
    pub input: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChatMessage {
    Text {
        role: MessageRole,
        content: Arc<str>,
    },
    Tool(ToolMessage),
    Status(String),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MessageRole {
    User,
    #[default]
    Chap,
    Error,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ToolMessage {
    pub call_id: String,
    pub name: String,
    pub arguments: Arc<str>,
    pub state: ToolState,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ToolState {
    #[default]
    Requested,
    Running,
    Interrupted,
    Finished(Result<Arc<str>, Arc<str>>),
}

impl ChatMessage {
    pub(super) fn user(content: String) -> Self {
        Self::Text {
            role: MessageRole::User,
            content: content.into(),
        }
    }

    pub(super) fn chap(content: String) -> Self {
        Self::Text {
            role: MessageRole::Chap,
            content: content.into(),
        }
    }

    pub(super) fn error(content: String) -> Self {
        Self::Text {
            role: MessageRole::Error,
            content: content.into(),
        }
    }

    fn status(content: impl Into<String>) -> Self {
        Self::Status(content.into())
    }

    fn tool(call_id: String, name: String, arguments: String, state: ToolState) -> Self {
        Self::Tool(ToolMessage {
            call_id,
            name,
            arguments: arguments.into(),
            state,
        })
    }
}

pub(super) fn apply_event(
    transcript: &mut TranscriptModel,
    event: SessionEventKind,
) -> Option<bool> {
    match event {
        SessionEventKind::RunStarted { input } => {
            transcript.messages.push(ChatMessage::user(input));
            Some(true)
        }
        SessionEventKind::SteeringQueued { id, input } => {
            transcript
                .pending_steering
                .push(PendingSteering { id, input });
            None
        }
        SessionEventKind::SteeringDiscarded { id, .. } => {
            remove_pending_steering(transcript, id);
            None
        }
        SessionEventKind::SteeringApplied { id, input } => {
            remove_pending_steering(transcript, id);
            transcript.messages.push(ChatMessage::user(input));
            None
        }
        SessionEventKind::AssistantMessage { text } => {
            transcript.messages.push(ChatMessage::chap(text));
            None
        }
        SessionEventKind::ToolRequested {
            call_id,
            name,
            arguments,
        } => {
            transcript.messages.push(ChatMessage::tool(
                call_id,
                name,
                arguments,
                ToolState::Requested,
            ));
            None
        }
        SessionEventKind::ToolStarted {
            call_id,
            name,
            arguments,
        } => {
            if let Some(tool) = tool_mut(&mut transcript.messages, &call_id) {
                tool.name = name;
                tool.arguments = arguments.into();
                tool.state = ToolState::Running;
            } else {
                transcript.messages.push(ChatMessage::tool(
                    call_id,
                    name,
                    arguments,
                    ToolState::Running,
                ));
            }
            None
        }
        SessionEventKind::ToolFinished {
            call_id,
            name,
            result,
        } => {
            if let Some(tool) = tool_mut(&mut transcript.messages, &call_id) {
                tool.name = name;
                tool.state = ToolState::Finished(shared_result(result));
            } else {
                transcript.messages.push(ChatMessage::tool(
                    call_id,
                    name,
                    String::new(),
                    ToolState::Finished(shared_result(result)),
                ));
            }
            None
        }
        SessionEventKind::ToolInterrupted { call_id, name } => {
            if let Some(tool) = tool_mut(&mut transcript.messages, &call_id) {
                tool.name = name;
                tool.state = ToolState::Interrupted;
            } else {
                transcript.messages.push(ChatMessage::tool(
                    call_id,
                    name,
                    String::new(),
                    ToolState::Interrupted,
                ));
            }
            None
        }
        SessionEventKind::UsageUpdated { usage } => {
            let model = transcript.usage.get_or_insert_default();
            // chap drops reasoning from assistant messages instead of sending it back to the
            // model. Counting output would include those billed tokens, so context reports the
            // last request's measured input rather than forecasting the next request. If chap
            // ever replays reasoning, this must become input plus output.
            model.context_tokens = usage.last_step.input_tokens;
            model.session.saturating_add_assign(usage.last_step);
            None
        }
        SessionEventKind::RunCompleted { .. } => Some(false),
        SessionEventKind::RunFailed { error } => {
            transcript.messages.push(ChatMessage::error(error));
            Some(false)
        }
        SessionEventKind::RunInterrupted => {
            transcript
                .messages
                .push(ChatMessage::status("run interrupted"));
            Some(false)
        }
        _ => None,
    }
}

fn remove_pending_steering(transcript: &mut TranscriptModel, id: SteeringId) {
    if let Some(index) = transcript
        .pending_steering
        .iter()
        .position(|steering| steering.id == id)
    {
        transcript.pending_steering.remove(index);
    }
}

fn tool_mut<'a>(messages: &'a mut [ChatMessage], call_id: &str) -> Option<&'a mut ToolMessage> {
    messages.iter_mut().rev().find_map(|message| match message {
        ChatMessage::Tool(tool) if tool.call_id == call_id => Some(tool),
        _ => None,
    })
}

fn shared_result(result: Result<String, String>) -> Result<Arc<str>, Arc<str>> {
    result.map(Into::into).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chap_core::{RunUsage, Usage};

    #[test]
    fn applies_run_and_tool_events_to_the_transcript() {
        let mut transcript = TranscriptModel::default();

        assert_eq!(
            apply_event(
                &mut transcript,
                SessionEventKind::RunStarted {
                    input: "hello".to_owned(),
                },
            ),
            Some(true)
        );
        apply_event(
            &mut transcript,
            SessionEventKind::ToolRequested {
                call_id: "call-1".to_owned(),
                name: "echo".to_owned(),
                arguments: "{}".to_owned(),
            },
        );
        apply_event(
            &mut transcript,
            SessionEventKind::ToolStarted {
                call_id: "call-1".to_owned(),
                name: "echo".to_owned(),
                arguments: r#"{"message":"hello"}"#.to_owned(),
            },
        );
        apply_event(
            &mut transcript,
            SessionEventKind::ToolFinished {
                call_id: "call-1".to_owned(),
                name: "echo".to_owned(),
                result: Ok("hello".to_owned()),
            },
        );
        apply_event(
            &mut transcript,
            SessionEventKind::AssistantMessage {
                text: "done".to_owned(),
            },
        );

        assert_eq!(
            transcript.messages,
            vec![
                ChatMessage::user("hello".to_owned()),
                ChatMessage::tool(
                    "call-1".to_owned(),
                    "echo".to_owned(),
                    r#"{"message":"hello"}"#.to_owned(),
                    ToolState::Finished(Ok(Arc::from("hello"))),
                ),
                ChatMessage::chap("done".to_owned()),
            ]
        );
        assert_eq!(
            apply_event(
                &mut transcript,
                SessionEventKind::RunCompleted {
                    response: "done".to_owned(),
                },
            ),
            Some(false)
        );
    }

    #[test]
    fn renders_run_failures_as_errors() {
        let mut transcript = TranscriptModel::default();

        assert_eq!(
            apply_event(
                &mut transcript,
                SessionEventKind::RunFailed {
                    error: "broken".to_owned(),
                },
            ),
            Some(false)
        );
        assert_eq!(
            transcript.messages,
            vec![ChatMessage::error("broken".to_owned())]
        );
    }

    #[test]
    fn renders_interrupted_runs_and_tools_as_terminal_states() {
        let mut transcript = TranscriptModel::default();
        apply_event(
            &mut transcript,
            SessionEventKind::ToolStarted {
                call_id: "call-1".to_owned(),
                name: "pause".to_owned(),
                arguments: "{}".to_owned(),
            },
        );
        apply_event(
            &mut transcript,
            SessionEventKind::ToolInterrupted {
                call_id: "call-1".to_owned(),
                name: "pause".to_owned(),
            },
        );

        assert_eq!(
            apply_event(&mut transcript, SessionEventKind::RunInterrupted),
            Some(false)
        );
        assert_eq!(
            transcript.messages,
            [
                ChatMessage::tool(
                    "call-1".to_owned(),
                    "pause".to_owned(),
                    "{}".to_owned(),
                    ToolState::Interrupted,
                ),
                ChatMessage::status("run interrupted"),
            ]
        );
    }

    #[test]
    fn accumulates_usage_and_measures_context_from_input() {
        let mut transcript = TranscriptModel::default();

        assert_eq!(
            apply_event(
                &mut transcript,
                SessionEventKind::UsageUpdated {
                    usage: RunUsage {
                        total: Usage {
                            input_tokens: 12,
                            cached_input_tokens: Some(4),
                            cache_write_tokens: Some(5),
                            output_tokens: 3,
                            reasoning_tokens: Some(2),
                        },
                        last_step: Usage {
                            input_tokens: 12,
                            cached_input_tokens: Some(4),
                            cache_write_tokens: Some(5),
                            output_tokens: 3,
                            reasoning_tokens: Some(2),
                        },
                    },
                },
            ),
            None
        );
        assert_eq!(
            transcript.usage,
            Some(UsageModel {
                context_tokens: 12,
                session: Usage {
                    input_tokens: 12,
                    cached_input_tokens: Some(4),
                    cache_write_tokens: Some(5),
                    output_tokens: 3,
                    reasoning_tokens: Some(2),
                },
            })
        );

        assert_eq!(
            apply_event(
                &mut transcript,
                SessionEventKind::UsageUpdated {
                    usage: RunUsage {
                        total: Usage {
                            input_tokens: 24,
                            cached_input_tokens: Some(10),
                            cache_write_tokens: Some(12),
                            output_tokens: 102,
                            reasoning_tokens: Some(5),
                        },
                        last_step: Usage {
                            input_tokens: 12,
                            cached_input_tokens: Some(6),
                            cache_write_tokens: Some(7),
                            output_tokens: 99,
                            reasoning_tokens: Some(3),
                        },
                    },
                },
            ),
            None
        );
        assert_eq!(
            transcript.usage,
            Some(UsageModel {
                context_tokens: 12,
                session: Usage {
                    input_tokens: 24,
                    cached_input_tokens: Some(10),
                    cache_write_tokens: Some(12),
                    output_tokens: 102,
                    reasoning_tokens: Some(5),
                },
            })
        );
    }

    #[test]
    fn keeps_unreported_cached_input_absent_in_footer_usage() {
        let mut transcript = TranscriptModel::default();

        apply_event(
            &mut transcript,
            SessionEventKind::UsageUpdated {
                usage: RunUsage {
                    total: Usage {
                        input_tokens: 900,
                        output_tokens: 300,
                        ..Usage::default()
                    },
                    last_step: Usage {
                        input_tokens: 900,
                        output_tokens: 300,
                        ..Usage::default()
                    },
                },
            },
        );

        assert_eq!(
            transcript.usage,
            Some(UsageModel {
                context_tokens: 900,
                session: Usage {
                    input_tokens: 900,
                    output_tokens: 300,
                    ..Usage::default()
                },
            })
        );
    }
}
