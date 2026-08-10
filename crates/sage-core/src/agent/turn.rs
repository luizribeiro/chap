use super::{MAX_PROVIDER_STEPS_PER_TURN, provider::CompletionBackend};
use crate::{
    session::{AssistantContent, Message, SessionEventKind, SessionState, ToolCall, ToolResult},
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
    session
        .messages
        .write()
        .await
        .push(Message::User(input.clone()));
    session.emit(SessionEventKind::RunStarted { input });

    let result = run_steps(session, tools, backend).await;
    match &result {
        Ok(response) => session.emit(SessionEventKind::RunCompleted {
            response: response.clone(),
        }),
        Err(error) => session.emit(SessionEventKind::RunFailed {
            error: error.clone(),
        }),
    }
    result
}

async fn run_steps(
    session: &SessionState,
    tools: &ToolRegistry,
    backend: &impl CompletionBackend,
) -> Result<String, String> {
    for _ in 0..MAX_PROVIDER_STEPS_PER_TURN {
        let messages = session.messages.read().await.clone();
        let completion = backend.complete(messages).await?;
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
            return text.ok_or_else(|| "provider returned a completion without text".to_owned());
        }

        for call in &tool_calls {
            session.emit(SessionEventKind::ToolRequested {
                call_id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            });
        }

        for call in tool_calls {
            session.emit(SessionEventKind::ToolStarted {
                call_id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            });
            let result = execute_tool(tools, call).await;
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
    }

    Err(format!(
        "turn exceeded the limit of {MAX_PROVIDER_STEPS_PER_TURN} provider requests"
    ))
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
