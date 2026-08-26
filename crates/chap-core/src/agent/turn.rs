use super::{
    MAX_PROVIDER_STEPS_PER_TURN, ToolExecutionConfig,
    provider::{CompletionBackend, FinishReason},
};
use crate::{
    session::{
        ActiveRun, AssistantContent, Message, RunBoundary, RunError, RunUsage, SessionEventKind,
        SessionState, Steering, ToolCall, ToolResult, Usage,
    },
    tool::{ExecutionMode, ToolRegistry},
};
use futures::{StreamExt, stream::FuturesUnordered};
use tokio::sync::Semaphore;

pub(super) async fn run_agent_loop(
    session: &SessionState,
    input: String,
    tools: &ToolRegistry,
    tool_execution: ToolExecutionConfig,
    backend: &impl CompletionBackend,
) -> Result<String, RunError> {
    if input.trim().is_empty() {
        return Err(RunError::Other("turn input cannot be empty".to_owned()));
    }

    let _run = session.turn_lock.lock().await;
    let mut active_run = session.start_run();
    session
        .messages
        .write()
        .await
        .push(Message::User(input.clone()));
    session.emit(SessionEventKind::RunStarted { input });

    let outcome = run_steps(session, tools, tool_execution, backend, &mut active_run).await;
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
            Err(RunError::Other("run interrupted".to_owned()))
        }
    }
}

enum RunOutcome {
    Completed(String),
    Failed(RunError),
    Interrupted,
}

impl From<Result<String, String>> for RunOutcome {
    fn from(result: Result<String, String>) -> Self {
        match result {
            Ok(response) => Self::Completed(response),
            Err(error) => Self::Failed(RunError::Other(error)),
        }
    }
}

async fn run_steps(
    session: &SessionState,
    tools: &ToolRegistry,
    tool_execution: ToolExecutionConfig,
    backend: &impl CompletionBackend,
    active_run: &mut ActiveRun<'_>,
) -> RunOutcome {
    let mut total_usage = Usage::default();
    let assembled_context = tokio::select! {
        biased;
        _ = active_run.interrupted() => return RunOutcome::Interrupted,
        context = backend.assemble_context(session.id()) => match context {
            Ok(context) => context,
            Err(error) => return RunOutcome::Failed(RunError::Other(error)),
        },
    };
    for _ in 0..MAX_PROVIDER_STEPS_PER_TURN {
        let mut messages = session.messages.read().await.clone();
        if let Some(context) = &assembled_context.context {
            messages.insert(0, Message::User(context.clone()));
        }
        if let Some(system) = &assembled_context.system {
            messages.insert(0, Message::System(system.clone()));
        }
        let completion = tokio::select! {
            biased;
            _ = active_run.interrupted() => return RunOutcome::Interrupted,
            completion = backend.complete(messages) => match completion {
                Ok(completion) => completion,
                Err(error) => return RunOutcome::Failed(RunError::Provider(error)),
            },
        };
        session
            .messages
            .write()
            .await
            .push(Message::Assistant(completion.content.clone()));
        if let Some(usage) = completion.usage {
            update_usage(session, &mut total_usage, usage);
        }

        let text = completion_text(&completion.content);
        if let Some(text) = &text {
            session.emit(SessionEventKind::AssistantMessage { text: text.clone() });
        }

        let tool_calls = completion
            .content
            .iter()
            .filter_map(|content| match content {
                AssistantContent::Text(_) => None,
                AssistantContent::Reasoning(_) => None,
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
                        .ok_or_else(|| completion_without_text_error(&completion.finish_reason))
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

        let mode = resolve_batch_mode(tools, &tool_calls, tool_execution.mode);
        let limit = match mode {
            ExecutionMode::Parallel => tool_execution.max_concurrency,
            ExecutionMode::Sequential => 1,
        };
        let slots = execute_tool_calls(session, tools, &tool_calls, active_run, limit).await;
        let mut interrupted = false;
        for (call, slot) in tool_calls.iter().zip(slots) {
            match slot {
                Some(result) => {
                    session
                        .messages
                        .write()
                        .await
                        .push(Message::ToolResult(result));
                }
                None => {
                    interrupted = true;
                    session
                        .messages
                        .write()
                        .await
                        .push(Message::ToolResult(ToolResult {
                            call_id: call.id.clone(),
                            name: call.name.clone(),
                            output: "run interrupted before tool completion".to_owned(),
                            is_error: true,
                        }));
                    session.emit(SessionEventKind::ToolInterrupted {
                        call_id: call.id.clone(),
                        name: call.name.clone(),
                    });
                }
            }
        }
        if interrupted {
            return RunOutcome::Interrupted;
        }

        let Some(steering) = session.take_steering() else {
            return RunOutcome::Interrupted;
        };
        append_steering(session, steering).await;
    }

    RunOutcome::Failed(RunError::Other(format!(
        "turn exceeded the limit of {MAX_PROVIDER_STEPS_PER_TURN} provider requests"
    )))
}

fn update_usage(session: &SessionState, total: &mut Usage, last_step: Usage) {
    total.saturating_add_assign(last_step);
    session.emit(SessionEventKind::UsageUpdated {
        usage: RunUsage {
            total: *total,
            last_step,
        },
    });
}

async fn execute_tool_calls(
    session: &SessionState,
    tools: &ToolRegistry,
    calls: &[ToolCall],
    active_run: &mut ActiveRun<'_>,
    limit: usize,
) -> Vec<Option<ToolResult>> {
    let semaphore = Semaphore::new(limit);
    let mut slots = (0..calls.len()).map(|_| None).collect::<Vec<_>>();
    let mut pending = calls
        .iter()
        .enumerate()
        .map(|(index, call)| {
            let semaphore = &semaphore;
            async move {
                let _permit = semaphore
                    .acquire()
                    .await
                    .expect("semaphore is never closed");
                session.emit(SessionEventKind::ToolStarted {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                });
                (index, execute_tool(tools, call.clone()).await)
            }
        })
        .collect::<FuturesUnordered<_>>();

    loop {
        tokio::select! {
            biased;
            _ = active_run.interrupted() => break,
            next = pending.next() => match next {
                Some((index, result)) => {
                    let event_result = if result.is_error {
                        Err(result.output.clone())
                    } else {
                        Ok(result.output.clone())
                    };
                    session.emit(SessionEventKind::ToolFinished {
                        call_id: result.call_id.clone(),
                        name: result.name.clone(),
                        result: event_result,
                    });
                    slots[index] = Some(result);
                }
                None => break,
            },
        }
    }

    slots
}

fn resolve_batch_mode(
    tools: &ToolRegistry,
    calls: &[ToolCall],
    session_mode: ExecutionMode,
) -> ExecutionMode {
    if session_mode == ExecutionMode::Sequential
        || calls
            .iter()
            .any(|call| tools.execution_mode(&call.name) == ExecutionMode::Sequential)
    {
        ExecutionMode::Sequential
    } else {
        ExecutionMode::Parallel
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
            AssistantContent::Reasoning(_) | AssistantContent::ToolCall(_) => None,
        })
        .collect::<Vec<_>>();
    if text.is_empty() {
        return None;
    }
    Some(text.join("\n"))
}

fn completion_without_text_error(finish_reason: &FinishReason) -> String {
    match finish_reason {
        FinishReason::Length => {
            "model reached its output limit before producing a response".to_owned()
        }
        FinishReason::Other(reason) => {
            format!("provider returned a completion without text (finish reason: {reason})")
        }
        FinishReason::Stop | FinishReason::ToolCalls => {
            "provider returned a completion without text".to_owned()
        }
    }
}
