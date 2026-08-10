use super::{MAX_PROVIDER_STEPS_PER_TURN, provider::CompletionBackend};
use crate::{
    session::{
        ActiveRun, AssistantContent, Message, RunBoundary, SessionEventKind, SessionState,
        Steering, ToolCall, ToolResult,
    },
    tool::ToolRegistry,
};

pub(super) async fn run_agent_loop(
    session: &SessionState,
    input: String,
    tools: &ToolRegistry,
    backend: &impl CompletionBackend,
) -> Result<String, String> {
    if input.trim().is_empty() {
        return Err("turn input cannot be empty".to_owned());
    }

    let _run = session.turn_lock.lock().await;
    let mut active_run = session.start_run();
    session
        .messages
        .write()
        .await
        .push(Message::User(input.clone()));
    session.emit(SessionEventKind::RunStarted { input });

    let outcome = run_steps(session, tools, backend, &mut active_run).await;
    drop(active_run);
    match outcome {
        RunOutcome::Completed(response) => {
            session.emit(SessionEventKind::RunCompleted {
                response: response.clone(),
            });
            Ok(response)
        }
        RunOutcome::Failed(error) => {
            session.emit(SessionEventKind::RunFailed {
                error: error.clone(),
            });
            Err(error)
        }
        RunOutcome::Interrupted => {
            session.emit(SessionEventKind::RunInterrupted);
            Err("run interrupted".to_owned())
        }
    }
}

enum RunOutcome {
    Completed(String),
    Failed(String),
    Interrupted,
}

impl From<Result<String, String>> for RunOutcome {
    fn from(result: Result<String, String>) -> Self {
        match result {
            Ok(response) => Self::Completed(response),
            Err(error) => Self::Failed(error),
        }
    }
}

async fn run_steps(
    session: &SessionState,
    tools: &ToolRegistry,
    backend: &impl CompletionBackend,
    active_run: &mut ActiveRun<'_>,
) -> RunOutcome {
    for _ in 0..MAX_PROVIDER_STEPS_PER_TURN {
        let messages = session.messages.read().await.clone();
        let completion = tokio::select! {
            biased;
            _ = active_run.interrupted() => return RunOutcome::Interrupted,
            completion = backend.complete(messages) => match completion {
                Ok(completion) => completion,
                Err(error) => return RunOutcome::Failed(error),
            },
        };
        session
            .messages
            .write()
            .await
            .push(Message::Assistant(completion.content.clone()));

        let text = completion_text(&completion.content);
        if let Some(text) = &text {
            session.emit(SessionEventKind::AssistantMessage { text: text.clone() });
        }

        let tool_calls = completion
            .content
            .iter()
            .filter_map(|content| match content {
                AssistantContent::Text(_) => None,
                AssistantContent::ToolCall(call) => Some(call.clone()),
            })
            .collect::<Vec<_>>();
        if tool_calls.is_empty() {
            match session.finish_or_take_steering() {
                RunBoundary::ApplySteering(steering) => {
                    append_steering(session, steering).await;
                    continue;
                }
                RunBoundary::Complete => {
                    return text
                        .ok_or_else(|| "provider returned a completion without text".to_owned())
                        .into();
                }
                RunBoundary::Interrupted => return RunOutcome::Interrupted,
            }
        }

        for call in &tool_calls {
            session.emit(SessionEventKind::ToolRequested {
                call_id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            });
        }

        for (index, call) in tool_calls.iter().cloned().enumerate() {
            session.emit(SessionEventKind::ToolStarted {
                call_id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            });
            let result = tokio::select! {
                biased;
                _ = active_run.interrupted() => {
                    interrupt_tools(session, &tool_calls[index..]).await;
                    return RunOutcome::Interrupted;
                }
                result = execute_tool(tools, call) => result,
            };
            let event_result = if result.is_error {
                Err(result.output.clone())
            } else {
                Ok(result.output.clone())
            };
            let call_id = result.call_id.clone();
            let name = result.name.clone();
            session
                .messages
                .write()
                .await
                .push(Message::ToolResult(result));
            session.emit(SessionEventKind::ToolFinished {
                call_id,
                name,
                result: event_result,
            });
        }

        let Some(steering) = session.take_steering() else {
            return RunOutcome::Interrupted;
        };
        append_steering(session, steering).await;
    }

    RunOutcome::Failed(format!(
        "turn exceeded the limit of {MAX_PROVIDER_STEPS_PER_TURN} provider requests"
    ))
}

async fn interrupt_tools(session: &SessionState, calls: &[ToolCall]) {
    {
        let mut messages = session.messages.write().await;
        messages.extend(calls.iter().map(|call| {
            Message::ToolResult(ToolResult {
                call_id: call.id.clone(),
                name: call.name.clone(),
                output: "run interrupted before tool completion".to_owned(),
                is_error: true,
            })
        }));
    }
    for call in calls {
        session.emit(SessionEventKind::ToolInterrupted {
            call_id: call.id.clone(),
            name: call.name.clone(),
        });
    }
}

async fn append_steering(session: &SessionState, steering: Vec<Steering>) {
    for steering in steering {
        session
            .messages
            .write()
            .await
            .push(Message::User(steering.input.clone()));
        session.emit(SessionEventKind::SteeringApplied {
            id: steering.id,
            input: steering.input,
        });
    }
}

async fn execute_tool(tools: &ToolRegistry, call: ToolCall) -> ToolResult {
    let output = tools.execute(&call.name, call.arguments).await;
    match output {
        Ok(output) => ToolResult {
            call_id: call.id,
            name: call.name,
            output,
            is_error: false,
        },
        Err(error) => ToolResult {
            call_id: call.id,
            name: call.name,
            output: error,
            is_error: true,
        },
    }
}

fn completion_text(content: &[AssistantContent]) -> Option<String> {
    let text = content
        .iter()
        .filter_map(|content| match content {
            AssistantContent::Text(text) => Some(text.as_str()),
            AssistantContent::ToolCall(_) => None,
        })
        .collect::<Vec<_>>();
    if text.is_empty() {
        return None;
    }
    Some(text.join("\n"))
}
