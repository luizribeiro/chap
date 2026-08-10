use super::{MAX_PROVIDER_STEPS_PER_TURN, provider::CompletionBackend};
use crate::{
    AssistantContent, Message, ToolCall, ToolResult, session::SessionState, tool::ToolRegistry,
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

    let _turn = session.turn_lock.lock().await;
    session.messages.write().await.push(Message::User(input));

    for _ in 0..MAX_PROVIDER_STEPS_PER_TURN {
        let messages = session.messages.read().await.clone();
        let completion = backend.complete(messages).await?;
        session
            .messages
            .write()
            .await
            .push(Message::Assistant(completion.content.clone()));

        let tool_calls = completion
            .content
            .iter()
            .filter_map(|content| match content {
                AssistantContent::Text(_) => None,
                AssistantContent::ToolCall(call) => Some(call.clone()),
            })
            .collect::<Vec<_>>();
        if tool_calls.is_empty() {
            return completion_text(&completion.content);
        }

        for call in tool_calls {
            let result = execute_tool(tools, call).await;
            session
                .messages
                .write()
                .await
                .push(Message::ToolResult(result));
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

fn completion_text(content: &[AssistantContent]) -> Result<String, String> {
    let text = content
        .iter()
        .filter_map(|content| match content {
            AssistantContent::Text(text) => Some(text.as_str()),
            AssistantContent::ToolCall(_) => None,
        })
        .collect::<Vec<_>>();
    if text.is_empty() {
        return Err("provider returned a completion without text".to_owned());
    }
    Ok(text.join("\n"))
}
