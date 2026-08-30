use super::super::{
    MAX_PROVIDER_STEPS_PER_TURN,
    provider::{CompletionBackend, CompletionFuture, ProviderCompletion},
    turn::run_agent_loop,
};
use crate::{
    ExecutionMode, FinishReason, ProviderError, RunError, SessionOptions, Tool, ToolDefinition,
    config::agent::ToolExecutionSettings,
    session::{
        AssembledContext, AssistantContent, Message, Reasoning, RunUsage, SessionEventKind,
        SessionEvents, SessionManager, SessionState, ToolCall, Usage,
    },
    tool::ToolRegistry,
};
use std::{
    collections::VecDeque,
    num::NonZeroUsize,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Barrier, Notify};

const TOOL_EXECUTION: ToolExecutionSettings = ToolExecutionSettings {
    mode: ExecutionMode::Parallel,
    max_concurrency: NonZeroUsize::new(8).expect("tool concurrency is nonzero"),
};

#[tokio::test]
async fn rejects_empty_turn_input() {
    let state = session_state();
    let backend = FakeBackend::new([]);

    let error = run_agent_loop(
        &state,
        " \n".to_owned(),
        &ToolRegistry::new(),
        TOOL_EXECUTION,
        &backend,
    )
    .await
    .unwrap_err();

    assert_eq!(error, RunError::EmptyInput);
    assert!(backend.requests.lock().unwrap().is_empty());
    assert!(state.messages.read().await.is_empty());
}

#[tokio::test]
async fn resumes_a_turn_after_executing_a_tool_call() {
    let state = session_state();
    let mut events = state.subscribe();
    let backend = FakeBackend::new([
        completion(vec![
            AssistantContent::Text("Let me check.".to_owned()),
            AssistantContent::ToolCall(ToolCall {
                id: "call-1".to_owned(),
                name: "echo".to_owned(),
                arguments: r#"{"message":"hello"}"#.to_owned(),
            }),
        ]),
        text_completion("The tool said hello."),
    ]);
    let mut tools = ToolRegistry::new();
    tools.register(EchoTool).unwrap();

    let response = run_agent_loop(
        &state,
        "say hello".to_owned(),
        &tools,
        TOOL_EXECUTION,
        &backend,
    )
    .await
    .unwrap();

    assert_eq!(response, "The tool said hello.");
    assert_eq!(
        receive_event_kinds(&mut events, 7).await,
        vec![
            SessionEventKind::RunStarted {
                input: "say hello".to_owned(),
            },
            SessionEventKind::AssistantMessage {
                text: "Let me check.".to_owned(),
            },
            SessionEventKind::ToolRequested {
                call_id: "call-1".to_owned(),
                name: "echo".to_owned(),
                arguments: r#"{"message":"hello"}"#.to_owned(),
            },
            SessionEventKind::ToolStarted {
                call_id: "call-1".to_owned(),
                name: "echo".to_owned(),
                arguments: r#"{"message":"hello"}"#.to_owned(),
            },
            SessionEventKind::ToolFinished {
                call_id: "call-1".to_owned(),
                name: "echo".to_owned(),
                result: Ok(r#"{"message":"hello"}"#.to_owned()),
            },
            SessionEventKind::AssistantMessage {
                text: "The tool said hello.".to_owned(),
            },
            SessionEventKind::RunCompleted {
                response: "The tool said hello.".to_owned(),
            },
        ]
    );
    {
        let requests = backend.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(matches!(
            requests[1].last(),
            Some(Message::ToolResult(result))
                if result.call_id == "call-1"
                    && result.name == "echo"
                    && result.output == r#"{"message":"hello"}"#
                    && !result.is_error
        ));
    }

    let history = state.messages.read().await;
    assert_eq!(history.len(), 4);
    assert!(matches!(history[0], Message::User(ref input) if input == "say hello"));
    assert!(matches!(
        history[1],
        Message::Assistant(ref content)
            if matches!(
                content.as_slice(),
                [AssistantContent::Text(text), AssistantContent::ToolCall(call)]
                    if text == "Let me check." && call.name == "echo"
            )
    ));
    assert!(matches!(history[2], Message::ToolResult(_)));
    assert!(matches!(
        history[3],
        Message::Assistant(ref content)
            if matches!(content.as_slice(), [AssistantContent::Text(text)] if text == "The tool said hello.")
    ));
}

#[tokio::test]
async fn stores_reasoning_in_session_history() {
    let state = session_state();
    let backend = FakeBackend::new([completion(vec![
        reasoning("I should answer directly.", Some("opaque-signature")),
        AssistantContent::Text("Hello.".to_owned()),
    ])]);

    assert_eq!(
        run_agent_loop(
            &state,
            "hello".to_owned(),
            &ToolRegistry::new(),
            TOOL_EXECUTION,
            &backend,
        )
        .await
        .unwrap(),
        "Hello."
    );
    assert_eq!(
        state.messages.read().await.as_slice(),
        [
            Message::User("hello".to_owned()),
            Message::Assistant(vec![
                reasoning("I should answer directly.", Some("opaque-signature")),
                AssistantContent::Text("Hello.".to_owned()),
            ]),
        ]
    );
}

#[tokio::test]
async fn carries_reasoning_into_the_following_provider_request() {
    let state = session_state();
    let backend = FakeBackend::new([
        completion(vec![
            reasoning("I need the tool.", Some("opaque-signature")),
            tool_call("call-1", "echo"),
        ]),
        text_completion("done"),
    ]);
    let mut tools = ToolRegistry::new();
    tools.register(EchoTool).unwrap();

    assert_eq!(
        run_agent_loop(&state, "hello".to_owned(), &tools, TOOL_EXECUTION, &backend)
            .await
            .unwrap(),
        "done"
    );
    let requests = backend.requests.lock().unwrap();
    assert_eq!(
        requests[1][1],
        Message::Assistant(vec![
            reasoning("I need the tool.", Some("opaque-signature")),
            tool_call("call-1", "echo"),
        ])
    );
}

#[tokio::test]
async fn emits_only_text_from_a_completion_with_reasoning() {
    let state = session_state();
    let mut events = state.subscribe();
    let backend = FakeBackend::new([completion(vec![
        reasoning("I should be concise.", None),
        AssistantContent::Text("Hello.".to_owned()),
    ])]);

    assert_eq!(
        run_agent_loop(
            &state,
            "hello".to_owned(),
            &ToolRegistry::new(),
            TOOL_EXECUTION,
            &backend,
        )
        .await
        .unwrap(),
        "Hello."
    );
    assert_eq!(
        receive_event_kinds(&mut events, 3).await,
        vec![
            SessionEventKind::RunStarted {
                input: "hello".to_owned(),
            },
            SessionEventKind::AssistantMessage {
                text: "Hello.".to_owned(),
            },
            SessionEventKind::RunCompleted {
                response: "Hello.".to_owned(),
            },
        ]
    );
}

#[tokio::test]
async fn emits_no_assistant_message_for_reasoning_alone() {
    let state = session_state();
    let mut events = state.subscribe();
    let backend = FakeBackend::new([completion(vec![reasoning("Still thinking.", None)])]);

    let error = run_agent_loop(
        &state,
        "hello".to_owned(),
        &ToolRegistry::new(),
        TOOL_EXECUTION,
        &backend,
    )
    .await
    .unwrap_err();

    assert_eq!(
        receive_event_kinds(&mut events, 2).await,
        vec![
            SessionEventKind::RunStarted {
                input: "hello".to_owned(),
            },
            SessionEventKind::RunFailed { error },
        ]
    );
}

#[tokio::test]
async fn accumulates_usage_across_provider_steps() {
    let state = session_state();
    let mut events = state.subscribe();
    let backend = FakeBackend::new([
        completion_with_detailed_usage(
            vec![tool_call("call-1", "echo")],
            Usage {
                input_tokens: 12,
                cached_input_tokens: Some(5),
                cache_write_tokens: Some(6),
                output_tokens: 4,
                reasoning_tokens: Some(2),
            },
        ),
        completion_with_detailed_usage(
            vec![AssistantContent::Text("done".to_owned())],
            Usage {
                input_tokens: 20,
                cached_input_tokens: None,
                cache_write_tokens: Some(7),
                output_tokens: 3,
                reasoning_tokens: None,
            },
        ),
    ]);
    let mut tools = ToolRegistry::new();
    tools.register(EchoTool).unwrap();

    assert_eq!(
        run_agent_loop(&state, "hello".to_owned(), &tools, TOOL_EXECUTION, &backend)
            .await
            .unwrap(),
        "done"
    );
    assert_eq!(
        receive_event_kinds(&mut events, 8).await,
        vec![
            SessionEventKind::RunStarted {
                input: "hello".to_owned(),
            },
            SessionEventKind::UsageUpdated {
                usage: RunUsage {
                    total: Usage {
                        input_tokens: 12,
                        cached_input_tokens: Some(5),
                        cache_write_tokens: Some(6),
                        output_tokens: 4,
                        reasoning_tokens: Some(2),
                    },
                    last_step: Usage {
                        input_tokens: 12,
                        cached_input_tokens: Some(5),
                        cache_write_tokens: Some(6),
                        output_tokens: 4,
                        reasoning_tokens: Some(2),
                    },
                },
            },
            SessionEventKind::ToolRequested {
                call_id: "call-1".to_owned(),
                name: "echo".to_owned(),
                arguments: "{}".to_owned(),
            },
            SessionEventKind::ToolStarted {
                call_id: "call-1".to_owned(),
                name: "echo".to_owned(),
                arguments: "{}".to_owned(),
            },
            SessionEventKind::ToolFinished {
                call_id: "call-1".to_owned(),
                name: "echo".to_owned(),
                result: Ok("{}".to_owned()),
            },
            SessionEventKind::UsageUpdated {
                usage: RunUsage {
                    total: Usage {
                        input_tokens: 32,
                        cached_input_tokens: Some(5),
                        cache_write_tokens: Some(13),
                        output_tokens: 7,
                        reasoning_tokens: Some(2),
                    },
                    last_step: Usage {
                        input_tokens: 20,
                        cached_input_tokens: None,
                        cache_write_tokens: Some(7),
                        output_tokens: 3,
                        reasoning_tokens: None,
                    },
                },
            },
            SessionEventKind::AssistantMessage {
                text: "done".to_owned(),
            },
            SessionEventKind::RunCompleted {
                response: "done".to_owned(),
            },
        ]
    );
}

#[tokio::test]
async fn emits_no_usage_events_when_unreported() {
    let state = session_state();
    let mut events = state.subscribe();
    let backend = FakeBackend::new([
        completion(vec![tool_call("call-1", "echo")]),
        text_completion("done"),
    ]);
    let mut tools = ToolRegistry::new();
    tools.register(EchoTool).unwrap();

    assert_eq!(
        run_agent_loop(&state, "hello".to_owned(), &tools, TOOL_EXECUTION, &backend)
            .await
            .unwrap(),
        "done"
    );
    assert_eq!(
        receive_event_kinds(&mut events, 6).await,
        vec![
            SessionEventKind::RunStarted {
                input: "hello".to_owned(),
            },
            SessionEventKind::ToolRequested {
                call_id: "call-1".to_owned(),
                name: "echo".to_owned(),
                arguments: "{}".to_owned(),
            },
            SessionEventKind::ToolStarted {
                call_id: "call-1".to_owned(),
                name: "echo".to_owned(),
                arguments: "{}".to_owned(),
            },
            SessionEventKind::ToolFinished {
                call_id: "call-1".to_owned(),
                name: "echo".to_owned(),
                result: Ok("{}".to_owned()),
            },
            SessionEventKind::AssistantMessage {
                text: "done".to_owned(),
            },
            SessionEventKind::RunCompleted {
                response: "done".to_owned(),
            },
        ]
    );
}

#[tokio::test]
async fn emits_usage_with_unreported_subset_counters() {
    let state = session_state();
    let mut events = state.subscribe();
    let backend = FakeBackend::new([
        completion_with_usage(vec![tool_call("call-1", "echo")], 12, 4),
        text_completion("done"),
    ]);
    let mut tools = ToolRegistry::new();
    tools.register(EchoTool).unwrap();

    assert_eq!(
        run_agent_loop(&state, "hello".to_owned(), &tools, TOOL_EXECUTION, &backend)
            .await
            .unwrap(),
        "done"
    );
    assert_eq!(
        receive_event_kinds(&mut events, 7).await,
        vec![
            SessionEventKind::RunStarted {
                input: "hello".to_owned(),
            },
            SessionEventKind::UsageUpdated {
                usage: RunUsage {
                    total: Usage {
                        input_tokens: 12,
                        cached_input_tokens: None,
                        cache_write_tokens: None,
                        output_tokens: 4,
                        reasoning_tokens: None,
                    },
                    last_step: Usage {
                        input_tokens: 12,
                        cached_input_tokens: None,
                        cache_write_tokens: None,
                        output_tokens: 4,
                        reasoning_tokens: None,
                    },
                },
            },
            SessionEventKind::ToolRequested {
                call_id: "call-1".to_owned(),
                name: "echo".to_owned(),
                arguments: "{}".to_owned(),
            },
            SessionEventKind::ToolStarted {
                call_id: "call-1".to_owned(),
                name: "echo".to_owned(),
                arguments: "{}".to_owned(),
            },
            SessionEventKind::ToolFinished {
                call_id: "call-1".to_owned(),
                name: "echo".to_owned(),
                result: Ok("{}".to_owned()),
            },
            SessionEventKind::AssistantMessage {
                text: "done".to_owned(),
            },
            SessionEventKind::RunCompleted {
                response: "done".to_owned(),
            },
        ]
    );
}

#[tokio::test]
async fn runs_independent_tool_calls_concurrently() {
    let state = session_state();
    let backend = FakeBackend::new([
        completion(vec![
            tool_call("call-1", "first"),
            tool_call("call-2", "second"),
        ]),
        text_completion("done"),
    ]);
    let first_release = Arc::new(Notify::new());
    let second_release = Arc::new(Notify::new());
    let mut tools = ToolRegistry::new();
    tools
        .register(CoordinatedTool {
            name: "first",
            mode: ExecutionMode::Parallel,
            behavior: ToolBehavior::SignalThenWait {
                signal: Arc::clone(&second_release),
                wait_for: Arc::clone(&first_release),
            },
        })
        .unwrap();
    tools
        .register(CoordinatedTool {
            name: "second",
            mode: ExecutionMode::Parallel,
            behavior: ToolBehavior::SignalThenWait {
                signal: first_release,
                wait_for: second_release,
            },
        })
        .unwrap();

    let response = tokio::time::timeout(
        Duration::from_secs(1),
        run_agent_loop(
            &state,
            "run both".to_owned(),
            &tools,
            TOOL_EXECUTION,
            &backend,
        ),
    )
    .await
    .expect("parallel tools should not deadlock")
    .unwrap();

    assert_eq!(response, "done");
}

#[tokio::test]
async fn records_tool_results_in_call_order_regardless_of_completion_order() {
    let state = session_state();
    let mut events = state.subscribe();
    let backend = FakeBackend::new([
        completion(vec![
            tool_call("call-1", "slow"),
            tool_call("call-2", "fast"),
        ]),
        text_completion("done"),
    ]);
    let fast_finished = Arc::new(Notify::new());
    let mut tools = ToolRegistry::new();
    tools
        .register(CoordinatedTool {
            name: "slow",
            mode: ExecutionMode::Parallel,
            behavior: ToolBehavior::WaitFor(Arc::clone(&fast_finished)),
        })
        .unwrap();
    tools
        .register(CoordinatedTool {
            name: "fast",
            mode: ExecutionMode::Parallel,
            behavior: ToolBehavior::Signal(fast_finished),
        })
        .unwrap();

    assert_eq!(
        run_agent_loop(
            &state,
            "run both".to_owned(),
            &tools,
            TOOL_EXECUTION,
            &backend,
        )
        .await
        .unwrap(),
        "done"
    );

    {
        let requests = backend.requests.lock().unwrap();
        let result_ids = requests[1]
            .iter()
            .filter_map(|message| match message {
                Message::ToolResult(result) => Some(result.call_id.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(result_ids, ["call-1", "call-2"]);
    }

    let finished_ids = receive_event_kinds(&mut events, 9)
        .await
        .into_iter()
        .filter_map(|kind| match kind {
            SessionEventKind::ToolFinished { call_id, .. } => Some(call_id),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(finished_ids, ["call-2", "call-1"]);
}

#[tokio::test]
async fn runs_the_batch_sequentially_when_a_tool_requires_it() {
    let state = session_state();
    let mut events = state.subscribe();
    let backend = FakeBackend::new([
        completion(vec![
            tool_call("call-1", "parallel"),
            tool_call("call-2", "sequential"),
        ]),
        text_completion("done"),
    ]);
    let active = Arc::new(AtomicUsize::new(0));
    let overlapped = Arc::new(AtomicBool::new(false));
    let mut tools = ToolRegistry::new();
    for (name, mode) in [
        ("parallel", ExecutionMode::Parallel),
        ("sequential", ExecutionMode::Sequential),
    ] {
        tools
            .register(CoordinatedTool {
                name,
                mode,
                behavior: ToolBehavior::TrackOverlap {
                    active: Arc::clone(&active),
                    overlapped: Arc::clone(&overlapped),
                },
            })
            .unwrap();
    }

    assert_eq!(
        run_agent_loop(
            &state,
            "run both".to_owned(),
            &tools,
            TOOL_EXECUTION,
            &backend,
        )
        .await
        .unwrap(),
        "done"
    );

    assert!(!overlapped.load(Ordering::SeqCst));
    let started_ids = receive_event_kinds(&mut events, 9)
        .await
        .into_iter()
        .filter_map(|kind| match kind {
            SessionEventKind::ToolStarted { call_id, .. } => Some(call_id),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(started_ids, ["call-1", "call-2"]);
}

#[tokio::test]
async fn applies_steering_before_the_next_provider_request() {
    let state = session_state();
    let mut events = state.subscribe();
    let backend = Arc::new(PausedBackend::new([
        text_completion("My first answer."),
        text_completion("My revised answer."),
    ]));
    let first_request = backend.first_request.notified();
    let run_state = Arc::clone(&state);
    let run_backend = Arc::clone(&backend);
    let run = tokio::spawn(async move {
        run_agent_loop(
            &run_state,
            "answer this".to_owned(),
            &ToolRegistry::new(),
            TOOL_EXECUTION,
            run_backend.as_ref(),
        )
        .await
    });

    first_request.await;
    let steering_id = state.steer("focus on the second part".to_owned()).unwrap();
    backend.release_first.notify_one();

    assert_eq!(run.await.unwrap().unwrap(), "My revised answer.");
    assert_eq!(
        receive_event_kinds(&mut events, 6).await,
        vec![
            SessionEventKind::RunStarted {
                input: "answer this".to_owned(),
            },
            SessionEventKind::SteeringQueued {
                id: steering_id,
                input: "focus on the second part".to_owned(),
            },
            SessionEventKind::AssistantMessage {
                text: "My first answer.".to_owned(),
            },
            SessionEventKind::SteeringApplied {
                id: steering_id,
                input: "focus on the second part".to_owned(),
            },
            SessionEventKind::AssistantMessage {
                text: "My revised answer.".to_owned(),
            },
            SessionEventKind::RunCompleted {
                response: "My revised answer.".to_owned(),
            },
        ]
    );

    let requests = backend.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(matches!(
        requests[1].as_slice(),
        [Message::User(initial), Message::Assistant(_), Message::User(steering)]
            if initial == "answer this" && steering == "focus on the second part"
    ));
}

#[tokio::test]
async fn preserves_interrupted_input_for_the_next_provider_request() {
    let state = session_state();
    let mut events = state.subscribe();
    let backend = Arc::new(PausedBackend::new([
        text_completion("too late"),
        text_completion("welcome back"),
    ]));
    let first_request = backend.first_request.notified();
    let run_state = Arc::clone(&state);
    let run_backend = Arc::clone(&backend);
    let run = tokio::spawn(async move {
        run_agent_loop(
            &run_state,
            "hello".to_owned(),
            &ToolRegistry::new(),
            TOOL_EXECUTION,
            run_backend.as_ref(),
        )
        .await
    });

    first_request.await;
    let steering_id = state.steer("queued detail".to_owned()).unwrap();
    state.interrupt().unwrap();

    assert_eq!(run.await.unwrap(), Err(RunError::Interrupted));
    assert_eq!(
        receive_event_kinds(&mut events, 4).await,
        vec![
            SessionEventKind::RunStarted {
                input: "hello".to_owned(),
            },
            SessionEventKind::SteeringQueued {
                id: steering_id,
                input: "queued detail".to_owned(),
            },
            SessionEventKind::SteeringDiscarded {
                id: steering_id,
                input: "queued detail".to_owned(),
            },
            SessionEventKind::RunInterrupted,
        ]
    );
    assert_eq!(
        *state.messages.read().await,
        [Message::User("hello".to_owned())]
    );

    assert_eq!(
        run_agent_loop(
            &state,
            "ops".to_owned(),
            &ToolRegistry::new(),
            TOOL_EXECUTION,
            backend.as_ref(),
        )
        .await
        .unwrap(),
        "welcome back"
    );
    let requests = backend.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(matches!(
        requests[1].as_slice(),
        [Message::User(interrupted), Message::User(next)]
            if interrupted == "hello" && next == "ops"
    ));
}

#[tokio::test]
async fn closes_unfinished_tool_calls_when_interrupted() {
    let state = session_state();
    let mut events = state.subscribe();
    let backend = FakeBackend::new([completion(vec![
        tool_call("call-1", "pause-1"),
        tool_call("call-2", "pause-2"),
    ])]);
    let started = Arc::new(Barrier::new(3));
    let mut tools = ToolRegistry::new();
    for name in ["pause-1", "pause-2"] {
        tools
            .register(CoordinatedTool {
                name,
                mode: ExecutionMode::Parallel,
                behavior: ToolBehavior::Pause(Arc::clone(&started)),
            })
            .unwrap();
    }
    let run_state = Arc::clone(&state);
    let run = tokio::spawn(async move {
        run_agent_loop(
            &run_state,
            "pause".to_owned(),
            &tools,
            TOOL_EXECUTION,
            &backend,
        )
        .await
    });

    tokio::time::timeout(Duration::from_secs(1), started.wait())
        .await
        .expect("both tools should start concurrently");
    state.interrupt().unwrap();

    assert_eq!(run.await.unwrap(), Err(RunError::Interrupted));
    assert_eq!(
        receive_event_kinds(&mut events, 8).await,
        vec![
            SessionEventKind::RunStarted {
                input: "pause".to_owned(),
            },
            SessionEventKind::ToolRequested {
                call_id: "call-1".to_owned(),
                name: "pause-1".to_owned(),
                arguments: "{}".to_owned(),
            },
            SessionEventKind::ToolRequested {
                call_id: "call-2".to_owned(),
                name: "pause-2".to_owned(),
                arguments: "{}".to_owned(),
            },
            SessionEventKind::ToolStarted {
                call_id: "call-1".to_owned(),
                name: "pause-1".to_owned(),
                arguments: "{}".to_owned(),
            },
            SessionEventKind::ToolStarted {
                call_id: "call-2".to_owned(),
                name: "pause-2".to_owned(),
                arguments: "{}".to_owned(),
            },
            SessionEventKind::ToolInterrupted {
                call_id: "call-1".to_owned(),
                name: "pause-1".to_owned(),
            },
            SessionEventKind::ToolInterrupted {
                call_id: "call-2".to_owned(),
                name: "pause-2".to_owned(),
            },
            SessionEventKind::RunInterrupted,
        ]
    );
    let history = state.messages.read().await;
    let results = history
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 2);
    for (result, (call_id, name)) in results
        .iter()
        .zip([("call-1", "pause-1"), ("call-2", "pause-2")])
    {
        assert_eq!(result.call_id, call_id);
        assert_eq!(result.name, name);
        assert_eq!(result.output, "run interrupted before tool completion");
        assert!(result.is_error);
    }
}

#[tokio::test]
async fn completes_finished_calls_when_interrupted_mid_batch() {
    let state = session_state();
    let mut events = state.subscribe();
    let backend = FakeBackend::new([completion(vec![
        tool_call("call-1", "finish"),
        tool_call("call-2", "pause"),
    ])]);
    let paused = Arc::new(Barrier::new(2));
    let mut tools = ToolRegistry::new();
    tools
        .register(CoordinatedTool {
            name: "finish",
            mode: ExecutionMode::Parallel,
            behavior: ToolBehavior::Immediate,
        })
        .unwrap();
    tools
        .register(CoordinatedTool {
            name: "pause",
            mode: ExecutionMode::Parallel,
            behavior: ToolBehavior::Pause(Arc::clone(&paused)),
        })
        .unwrap();
    let run_state = Arc::clone(&state);
    let run = tokio::spawn(async move {
        run_agent_loop(
            &run_state,
            "run both".to_owned(),
            &tools,
            TOOL_EXECUTION,
            &backend,
        )
        .await
    });

    tokio::time::timeout(Duration::from_secs(1), paused.wait())
        .await
        .expect("the unfinished tool should start");
    state.interrupt().unwrap();

    assert_eq!(run.await.unwrap(), Err(RunError::Interrupted));
    let history = state.messages.read().await;
    let results = history
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].call_id, "call-1");
    assert_eq!(results[0].output, "finish");
    assert!(!results[0].is_error);
    assert_eq!(results[1].call_id, "call-2");
    assert_eq!(results[1].output, "run interrupted before tool completion");
    assert!(results[1].is_error);

    let kinds = receive_event_kinds(&mut events, 8).await;
    assert!(kinds.iter().any(|kind| matches!(
        kind,
        SessionEventKind::ToolFinished { call_id, .. } if call_id == "call-1"
    )));
    assert!(kinds.iter().any(|kind| matches!(
        kind,
        SessionEventKind::ToolInterrupted { call_id, .. } if call_id == "call-2"
    )));
    assert!(!kinds.iter().any(|kind| matches!(
        kind,
        SessionEventKind::ToolInterrupted { call_id, .. } if call_id == "call-1"
    )));
}

#[tokio::test]
async fn returns_tool_failures_to_the_provider() {
    let state = session_state();
    let mut events = state.subscribe();
    let backend = FakeBackend::new([
        completion(vec![AssistantContent::ToolCall(ToolCall {
            id: "call-1".to_owned(),
            name: "missing".to_owned(),
            arguments: "{}".to_owned(),
        })]),
        text_completion("I could not run that tool."),
    ]);

    let response = run_agent_loop(
        &state,
        "use a missing tool".to_owned(),
        &ToolRegistry::new(),
        TOOL_EXECUTION,
        &backend,
    )
    .await
    .unwrap();

    assert_eq!(response, "I could not run that tool.");
    let events = receive_event_kinds(&mut events, 6).await;
    assert_eq!(
        events[3],
        SessionEventKind::ToolFinished {
            call_id: "call-1".to_owned(),
            name: "missing".to_owned(),
            result: Err("tool `missing` is not registered".to_owned()),
        }
    );
    let requests = backend.requests.lock().unwrap();
    assert!(matches!(
        requests[1].last(),
        Some(Message::ToolResult(result))
            if result.output == "tool `missing` is not registered" && result.is_error
    ));
}

#[tokio::test]
async fn preserves_the_provider_error_in_the_failed_terminal_event() {
    let state = session_state();
    let mut events = state.subscribe();

    let provider_error = ProviderError::RateLimited {
        retry_after: Some(Duration::from_secs(30)),
        message: "slow down".to_owned(),
    };
    let error = run_agent_loop(
        &state,
        "hello".to_owned(),
        &ToolRegistry::new(),
        TOOL_EXECUTION,
        &FailingBackend(provider_error.clone()),
    )
    .await
    .unwrap_err();

    assert_eq!(error, RunError::Provider(provider_error));
    assert_eq!(
        receive_event_kinds(&mut events, 2).await,
        vec![
            SessionEventKind::RunStarted {
                input: "hello".to_owned(),
            },
            SessionEventKind::RunFailed { error },
        ]
    );
}

#[tokio::test]
async fn explains_when_an_empty_completion_reached_the_output_limit() {
    assert_eq!(
        empty_completion_error(FinishReason::Length).await,
        RunError::CompletionWithoutText {
            finish_reason: FinishReason::Length,
        }
    );
}

#[tokio::test]
async fn keeps_the_empty_completion_error_for_a_stop_finish_reason() {
    assert_eq!(
        empty_completion_error(FinishReason::Stop).await,
        RunError::CompletionWithoutText {
            finish_reason: FinishReason::Stop,
        }
    );
}

#[tokio::test]
async fn keeps_the_tool_calls_finish_reason_on_an_empty_completion() {
    assert_eq!(
        empty_completion_error(FinishReason::ToolCalls).await,
        RunError::CompletionWithoutText {
            finish_reason: FinishReason::ToolCalls,
        }
    );
}

#[tokio::test]
async fn includes_an_other_finish_reason_in_the_empty_completion_error() {
    assert_eq!(
        empty_completion_error(FinishReason::Other("content_filter".to_owned())).await,
        RunError::CompletionWithoutText {
            finish_reason: FinishReason::Other("content_filter".to_owned()),
        }
    );
}

#[tokio::test]
async fn reports_the_provider_step_limit_structurally() {
    let state = session_state();
    let backend = FakeBackend::new(
        (0..MAX_PROVIDER_STEPS_PER_TURN)
            .map(|index| completion(vec![tool_call(&format!("call-{index}"), "echo")])),
    );
    let mut tools = ToolRegistry::new();
    tools.register(EchoTool).unwrap();

    let error = run_agent_loop(
        &state,
        "keep going".to_owned(),
        &tools,
        TOOL_EXECUTION,
        &backend,
    )
    .await
    .unwrap_err();

    assert_eq!(
        error,
        RunError::ProviderStepLimitExceeded {
            limit: MAX_PROVIDER_STEPS_PER_TURN,
        }
    );
}

#[tokio::test]
async fn prepends_the_session_context_to_every_provider_request() {
    let state = session_state_with_context(AssembledContext {
        system: Some("operator guidance".to_owned()),
        context: Some("project notes".to_owned()),
    });
    let backend = FakeBackend::new([
        completion(vec![tool_call("call-1", "echo")]),
        text_completion("done"),
    ]);
    let mut tools = ToolRegistry::new();
    tools.register(EchoTool).unwrap();

    run_agent_loop(&state, "hello".to_owned(), &tools, TOOL_EXECUTION, &backend)
        .await
        .unwrap();

    {
        let requests = backend.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        for request in requests.iter() {
            assert_eq!(
                request[..3],
                [
                    Message::System("operator guidance".to_owned()),
                    Message::User("project notes".to_owned()),
                    Message::User("hello".to_owned()),
                ]
            );
        }
    }

    let history = state.messages.read().await;
    assert!(
        !history.iter().any(|message| match message {
            Message::System(_) => true,
            Message::User(input) => input == "project notes",
            _ => false,
        }),
        "assembled context must never be stored in session history: {history:?}"
    );
}

fn session_state() -> Arc<SessionState> {
    session_state_with_context(AssembledContext::default())
}

fn session_state_with_context(context: AssembledContext) -> Arc<SessionState> {
    SessionManager::new()
        .create(SessionOptions::new("test-provider"), context)
        .unwrap()
}

async fn receive_event_kinds(events: &mut SessionEvents, count: usize) -> Vec<SessionEventKind> {
    let mut kinds = Vec::with_capacity(count);
    for expected_sequence in 1..=count as u64 {
        let event = events.recv().await.unwrap().unwrap();
        assert_eq!(event.sequence, expected_sequence);
        kinds.push(event.kind);
    }
    kinds
}

fn completion(content: Vec<AssistantContent>) -> ProviderCompletion {
    ProviderCompletion {
        content,
        finish_reason: FinishReason::Stop,
        usage: None,
    }
}

fn text_completion(text: &str) -> ProviderCompletion {
    completion(vec![AssistantContent::Text(text.to_owned())])
}

fn completion_with_usage(
    content: Vec<AssistantContent>,
    input_tokens: u64,
    output_tokens: u64,
) -> ProviderCompletion {
    let mut completion = completion(content);
    completion.usage = Some(Usage {
        input_tokens,
        output_tokens,
        ..Usage::default()
    });
    completion
}

fn completion_with_detailed_usage(
    content: Vec<AssistantContent>,
    usage: Usage,
) -> ProviderCompletion {
    let mut completion = completion(content);
    completion.usage = Some(usage);
    completion
}

fn completion_with_finish_reason(
    content: Vec<AssistantContent>,
    finish_reason: FinishReason,
) -> ProviderCompletion {
    let mut completion = completion(content);
    completion.finish_reason = finish_reason;
    completion
}

async fn empty_completion_error(finish_reason: FinishReason) -> RunError {
    let state = session_state();
    let backend = FakeBackend::new([completion_with_finish_reason(Vec::new(), finish_reason)]);

    run_agent_loop(
        &state,
        "hello".to_owned(),
        &ToolRegistry::new(),
        TOOL_EXECUTION,
        &backend,
    )
    .await
    .unwrap_err()
}

fn tool_call(id: &str, name: &str) -> AssistantContent {
    AssistantContent::ToolCall(ToolCall {
        id: id.to_owned(),
        name: name.to_owned(),
        arguments: "{}".to_owned(),
    })
}

fn reasoning(text: &str, signature: Option<&str>) -> AssistantContent {
    AssistantContent::Reasoning(Reasoning {
        text: text.to_owned(),
        signature: signature.map(str::to_owned),
    })
}

struct FakeBackend {
    completions: Mutex<VecDeque<ProviderCompletion>>,
    requests: Mutex<Vec<Vec<Message>>>,
}

struct FailingBackend(ProviderError);

impl CompletionBackend for FailingBackend {
    fn complete(&self, _messages: Vec<Message>) -> CompletionFuture<'_> {
        let error = self.0.clone();
        Box::pin(async move { Err(error) })
    }
}

struct PausedBackend {
    completions: Mutex<VecDeque<ProviderCompletion>>,
    requests: Mutex<Vec<Vec<Message>>>,
    first_request: Notify,
    release_first: Notify,
}

impl PausedBackend {
    fn new(completions: impl IntoIterator<Item = ProviderCompletion>) -> Self {
        Self {
            completions: Mutex::new(completions.into_iter().collect()),
            requests: Mutex::new(Vec::new()),
            first_request: Notify::new(),
            release_first: Notify::new(),
        }
    }
}

impl CompletionBackend for PausedBackend {
    fn complete(&self, messages: Vec<Message>) -> CompletionFuture<'_> {
        let is_first = self.requests.lock().unwrap().is_empty();
        self.requests.lock().unwrap().push(messages);
        let completion = self.completions.lock().unwrap().pop_front().ok_or_else(|| {
            ProviderError::Other("paused provider ran out of completions".to_owned())
        });

        Box::pin(async move {
            if is_first {
                self.first_request.notify_one();
                self.release_first.notified().await;
            }
            completion
        })
    }
}

impl FakeBackend {
    fn new(completions: impl IntoIterator<Item = ProviderCompletion>) -> Self {
        Self {
            completions: Mutex::new(completions.into_iter().collect()),
            requests: Mutex::new(Vec::new()),
        }
    }
}

impl CompletionBackend for FakeBackend {
    fn complete(&self, messages: Vec<Message>) -> CompletionFuture<'_> {
        self.requests.lock().unwrap().push(messages);
        let completion =
            self.completions.lock().unwrap().pop_front().ok_or_else(|| {
                ProviderError::Other("fake provider ran out of completions".to_owned())
            });
        Box::pin(async move { completion })
    }
}

struct EchoTool;

struct CoordinatedTool {
    name: &'static str,
    mode: ExecutionMode,
    behavior: ToolBehavior,
}

enum ToolBehavior {
    Immediate,
    Pause(Arc<Barrier>),
    Signal(Arc<Notify>),
    SignalThenWait {
        signal: Arc<Notify>,
        wait_for: Arc<Notify>,
    },
    TrackOverlap {
        active: Arc<AtomicUsize>,
        overlapped: Arc<AtomicBool>,
    },
    WaitFor(Arc<Notify>),
}

impl Tool for CoordinatedTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name.to_owned(),
            description: "Coordinates test execution".to_owned(),
            parameters: r#"{"type":"object"}"#.to_owned(),
        }
    }

    fn execution_mode(&self) -> ExecutionMode {
        self.mode
    }

    fn execute(
        &self,
        _arguments: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
    {
        Box::pin(async move {
            match &self.behavior {
                ToolBehavior::Immediate => {}
                ToolBehavior::Pause(started) => {
                    started.wait().await;
                    std::future::pending::<()>().await;
                }
                ToolBehavior::Signal(signal) => signal.notify_one(),
                ToolBehavior::SignalThenWait { signal, wait_for } => {
                    signal.notify_one();
                    wait_for.notified().await;
                }
                ToolBehavior::TrackOverlap { active, overlapped } => {
                    if active.fetch_add(1, Ordering::SeqCst) != 0 {
                        overlapped.store(true, Ordering::SeqCst);
                    }
                    tokio::task::yield_now().await;
                    active.fetch_sub(1, Ordering::SeqCst);
                }
                ToolBehavior::WaitFor(notification) => notification.notified().await,
            }
            Ok(self.name.to_owned())
        })
    }
}

impl Tool for EchoTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "echo".to_owned(),
            description: "Returns its arguments".to_owned(),
            parameters: r#"{"type":"object"}"#.to_owned(),
        }
    }

    fn execute(
        &self,
        arguments: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
    {
        Box::pin(async move { Ok(arguments) })
    }
}
