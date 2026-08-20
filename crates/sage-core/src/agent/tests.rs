use super::{
    AgentBuilder,
    provider::{CompletionBackend, CompletionFuture, ProviderCompletion},
    turn::run_agent_loop,
};
use crate::{
    SessionOptions, Tool, ToolDefinition,
    session::{
        AssistantContent, Message, SessionEventKind, SessionEvents, SessionManager, ToolCall,
    },
    tool::ToolRegistry,
};
use lockgate::{ConsentRequired, DriftReport};
use std::{
    collections::VecDeque,
    fs,
    path::Path,
    sync::{Arc, Mutex, mpsc},
};
use tokio::sync::Notify;
use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
use wit_parser::{ManglingAndAbi, Resolve};

#[tokio::test]
async fn approved_matching_manifest_admits_a_configured_provider() {
    let directory = test_directory();
    let component = directory.join("provider.wasm");
    fs::write(
        &component,
        provider_component_with_schema(
            "example.provider",
            r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"model":{"type":"string"}},"required":["model"],"additionalProperties":false}"#,
        ),
    )
    .unwrap();
    let config_path = directory.join("sage.toml");
    fs::write(
        &config_path,
        r#"
[plugins."example.provider"]
component = "provider.wasm"

[plugins."example.provider".settings]
model = "example-model"
"#,
    )
    .unwrap();

    let builder = AgentBuilder::load(&config_path).unwrap();

    assert_eq!(builder.plugins().count(), 1);
    assert_eq!(
        builder.plugin_roles("example.provider").unwrap(),
        ["provider"]
    );
    let record = builder.approve_plugin("example.provider").await.unwrap();
    assert!(
        time::OffsetDateTime::parse(
            &record.approved_at,
            &time::format_description::well_known::Rfc3339
        )
        .is_ok()
    );
    let agent = builder.start().await.unwrap();
    assert!(agent.plugin_errors().next().is_none());
    assert!(agent.inner.plugins.contains_key("example.provider"));
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn first_run_refuses_only_the_unapproved_plugin() {
    let directory = test_directory();
    fs::write(
        directory.join("provider.wasm"),
        provider_component("example.provider"),
    )
    .unwrap();
    let config_path = directory.join("sage.toml");
    fs::write(
        &config_path,
        r#"
[plugins.approved]
component = "provider.wasm"

[plugins.unapproved]
component = "provider.wasm"
"#,
    )
    .unwrap();

    let builder = AgentBuilder::load(&config_path).unwrap();
    builder.approve_plugin("approved").await.unwrap();
    let agent = builder.start().await.unwrap();
    let errors = agent.plugin_errors().collect::<Vec<_>>();

    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].0, "unapproved");
    assert!(errors[0].1.contains("requires approval"), "{}", errors[0].1);
    assert!(
        errors[0].1.contains("sage grants review unapproved"),
        "{}",
        errors[0].1
    );
    assert!(agent.inner.plugins.contains_key("approved"));
    assert!(!agent.inner.plugins.contains_key("unapproved"));
    assert_eq!(
        agent
            .session(SessionOptions::new("unapproved"))
            .err()
            .unwrap(),
        errors[0].1
    );
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn approving_then_denying_toggles_plugin_admission() {
    let directory = test_directory();
    fs::write(
        directory.join("provider.wasm"),
        provider_component("example.provider"),
    )
    .unwrap();
    let config_path = directory.join("sage.toml");
    fs::write(
        &config_path,
        r#"
[plugins.example]
component = "provider.wasm"
"#,
    )
    .unwrap();

    let builder = AgentBuilder::load(&config_path).unwrap();
    assert!(
        builder
            .review_plugin("example")
            .await
            .unwrap()
            .prior
            .is_none()
    );
    builder.approve_plugin("example").await.unwrap();
    let agent = builder.start().await.unwrap();
    assert!(agent.plugin_errors().next().is_none());
    tokio::task::spawn_blocking(move || drop(agent))
        .await
        .unwrap();

    let builder = AgentBuilder::load(&config_path).unwrap();
    builder.deny_plugin("example").unwrap();
    assert!(
        builder
            .review_plugin("example")
            .await
            .unwrap()
            .prior
            .is_none()
    );
    let agent = builder.start().await.unwrap();
    assert!(
        agent
            .plugin_errors()
            .any(|(id, error)| id == "example" && error.contains("requires approval"))
    );
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn nonblocking_drift_errors_are_reported_without_panicking() {
    let directory = test_directory();
    let component = directory.join("provider.wasm");
    fs::write(&component, provider_component("example.provider")).unwrap();
    let config_path = directory.join("sage.toml");
    fs::write(
        &config_path,
        r#"
[plugins.example]
component = "provider.wasm"
"#,
    )
    .unwrap();
    let builder = AgentBuilder::load(&config_path).unwrap();
    let manifest = builder.review_plugin("example").await.unwrap().manifest;

    let error = AgentBuilder::consent_error(
        "example",
        &component,
        ConsentRequired::Drift {
            manifest,
            drift: DriftReport {
                changes: Vec::new(),
                blocks_admission: false,
            },
        },
    );

    assert!(
        error.contains("reported a changed permission manifest"),
        "{error}"
    );
    assert!(error.contains("sage grants review example"), "{error}");
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn rejects_missing_required_settings_during_prepare() {
    let directory = test_directory();
    let component = directory.join("provider.wasm");
    fs::write(
        &component,
        provider_component_with_schema(
            "example.provider",
            r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"model":{"type":"string"}},"required":["model"],"additionalProperties":false}"#,
        ),
    )
    .unwrap();
    let config_path = directory.join("sage.toml");
    fs::write(
        &config_path,
        r#"
[plugins."example.provider"]
component = "provider.wasm"
"#,
    )
    .unwrap();

    let error = match AgentBuilder::load(&config_path).unwrap().start().await {
        Ok(_) => panic!("missing required settings should be rejected"),
        Err(error) => error,
    };

    assert!(error.contains("settings"), "{error}");
    assert!(error.contains("model"), "{error}");
    assert!(error.contains("required"), "{error}");
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn validates_settings_before_loading_tool_definitions() {
    let directory = test_directory();
    let component = directory.join("tools.wasm");
    fs::write(
        &component,
        tool_component_with_schema(
            "example.tools",
            r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","required":["api-key"]}"#,
        ),
    )
    .unwrap();
    let config_path = directory.join("sage.toml");
    fs::write(
        &config_path,
        r#"
[plugins."example.tools"]
component = "tools.wasm"
"#,
    )
    .unwrap();

    let error = match AgentBuilder::load(&config_path).unwrap().start().await {
        Ok(_) => panic!("invalid tool settings should be rejected"),
        Err(error) => error,
    };

    assert!(error.contains("settings"), "{error}");
    assert!(error.contains("api-key"), "{error}");
    assert!(error.contains("required"), "{error}");
    assert!(!error.contains("tool plugin"));
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn reports_framework_schema_transport_errors() {
    let directory = test_directory();
    let config_path = directory.join("sage.toml");
    fs::write(
        &config_path,
        r#"
[plugins.example]
component = "provider.wasm"
"#,
    )
    .unwrap();

    fs::write(
        directory.join("provider.wasm"),
        provider_component_with_trapping_schema("example"),
    )
    .unwrap();
    let transport = match AgentBuilder::load(&config_path).unwrap().start().await {
        Ok(_) => panic!("a trapping schema export should be rejected"),
        Err(error) => error,
    };
    assert!(transport.contains("settings schema"), "{transport}");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn discovers_a_configured_tool_plugin() {
    let directory = test_directory();
    let component = directory.join("tools.wasm");
    fs::write(&component, tool_component("example.tools")).unwrap();
    let config_path = directory.join("sage.toml");
    fs::write(
        &config_path,
        r#"
[plugins."example.tools"]
component = "tools.wasm"
"#,
    )
    .unwrap();

    let builder = AgentBuilder::load(&config_path).unwrap();

    assert_eq!(builder.plugin_roles("example.tools").unwrap(), ["tool"]);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn loads_definitions_from_an_admitted_tool_plugin() {
    let directory = test_directory();
    fs::write(
        directory.join("tools.wasm"),
        tool_component("example.tools"),
    )
    .unwrap();
    let config_path = directory.join("sage.toml");
    fs::write(
        &config_path,
        r#"
[plugins."example.tools"]
component = "tools.wasm"
"#,
    )
    .unwrap();

    let builder = AgentBuilder::load(&config_path).unwrap();
    builder.approve_plugin("example.tools").await.unwrap();
    let agent = builder.start().await.unwrap();

    assert!(agent.plugin_errors().next().is_none());
    assert!(agent.tool_definitions().is_empty());
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn accepts_an_instance_id_that_differs_from_plugin_metadata() {
    let directory = test_directory();
    let component = directory.join("provider.wasm");
    fs::write(&component, provider_component("embedded.id")).unwrap();
    let config_path = directory.join("sage.toml");
    fs::write(
        &config_path,
        r#"
[plugins.config-id]
component = "provider.wasm"
"#,
    )
    .unwrap();

    let builder = AgentBuilder::load(&config_path).unwrap();
    builder.approve_plugin("config-id").await.unwrap();
    let agent = builder.start().await.unwrap();
    assert!(agent.plugin_errors().next().is_none());
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn drops_partial_start_resources_on_a_blocking_thread() {
    let directory = test_directory();
    fs::write(
        directory.join("provider.wasm"),
        provider_component("example.provider"),
    )
    .unwrap();
    let config_path = directory.join("sage.toml");
    fs::write(
        &config_path,
        r#"
[plugins.a-provider]
component = "provider.wasm"

[plugins.z-missing]
component = "missing.wasm"
"#,
    )
    .unwrap();
    let (dropped, observed_drop) = mpsc::sync_channel(1);
    let async_thread = std::thread::current().id();
    let builder = AgentBuilder::load(&config_path)
        .unwrap()
        .tool(DropProbe { dropped })
        .unwrap();
    builder.approve_plugin("a-provider").await.unwrap();

    let error = match builder.start().await {
        Ok(_) => panic!("the missing second plugin should fail admission"),
        Err(error) => error,
    };

    assert!(error.contains("z-missing"), "{error}");
    assert_ne!(observed_drop.recv().unwrap(), async_thread);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn reports_the_component_path_for_an_unsupported_plugin_role() {
    let directory = test_directory();
    let component = directory.join("unsupported.wasm");
    fs::write(&component, unsupported_component("example.unsupported")).unwrap();
    let config_path = directory.join("sage.toml");
    fs::write(
        &config_path,
        r#"
[plugins.example]
component = "unsupported.wasm"
"#,
    )
    .unwrap();

    let builder = AgentBuilder::load(&config_path).unwrap();
    builder.approve_plugin("example").await.unwrap();
    let error = match builder.start().await {
        Ok(_) => panic!("a plugin without a supported role should be rejected"),
        Err(error) => error,
    };

    assert_eq!(
        error,
        format!(
            "plugin `example` from `{}` does not implement a supported role",
            component.display()
        )
    );
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn resumes_a_turn_after_executing_a_tool_call() {
    let manager = SessionManager::new();
    let state = manager
        .create(SessionOptions::new("test-provider"))
        .unwrap();
    let mut events = state.subscribe();
    let backend = FakeBackend::new([
        ProviderCompletion {
            content: vec![
                AssistantContent::Text("Let me check.".to_owned()),
                AssistantContent::ToolCall(ToolCall {
                    id: "call-1".to_owned(),
                    name: "echo".to_owned(),
                    arguments: r#"{"message":"hello"}"#.to_owned(),
                }),
            ],
        },
        ProviderCompletion {
            content: vec![AssistantContent::Text("The tool said hello.".to_owned())],
        },
    ]);
    let mut tools = ToolRegistry::new();
    tools.register(EchoTool).unwrap();

    let response = run_agent_loop(&state, "say hello".to_owned(), &tools, &backend)
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
async fn applies_steering_before_the_next_provider_request() {
    let manager = SessionManager::new();
    let state = manager
        .create(SessionOptions::new("test-provider"))
        .unwrap();
    let mut events = state.subscribe();
    let backend = Arc::new(PausedBackend::new([
        ProviderCompletion {
            content: vec![AssistantContent::Text("My first answer.".to_owned())],
        },
        ProviderCompletion {
            content: vec![AssistantContent::Text("My revised answer.".to_owned())],
        },
    ]));
    let first_request = backend.first_request.notified();
    let run_state = Arc::clone(&state);
    let run_backend = Arc::clone(&backend);
    let run = tokio::spawn(async move {
        run_agent_loop(
            &run_state,
            "answer this".to_owned(),
            &ToolRegistry::new(),
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
    let manager = SessionManager::new();
    let state = manager
        .create(SessionOptions::new("test-provider"))
        .unwrap();
    let mut events = state.subscribe();
    let backend = Arc::new(PausedBackend::new([
        ProviderCompletion {
            content: vec![AssistantContent::Text("too late".to_owned())],
        },
        ProviderCompletion {
            content: vec![AssistantContent::Text("welcome back".to_owned())],
        },
    ]));
    let first_request = backend.first_request.notified();
    let run_state = Arc::clone(&state);
    let run_backend = Arc::clone(&backend);
    let run = tokio::spawn(async move {
        run_agent_loop(
            &run_state,
            "hello".to_owned(),
            &ToolRegistry::new(),
            run_backend.as_ref(),
        )
        .await
    });

    first_request.await;
    let steering_id = state.steer("queued detail".to_owned()).unwrap();
    state.interrupt().unwrap();

    assert_eq!(run.await.unwrap(), Err("run interrupted".to_owned()));
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
    let manager = SessionManager::new();
    let state = manager
        .create(SessionOptions::new("test-provider"))
        .unwrap();
    let mut events = state.subscribe();
    let backend = FakeBackend::new([ProviderCompletion {
        content: vec![AssistantContent::ToolCall(ToolCall {
            id: "call-1".to_owned(),
            name: "pause".to_owned(),
            arguments: "{}".to_owned(),
        })],
    }]);
    let started = Arc::new(Notify::new());
    let tool_started = started.notified();
    let mut tools = ToolRegistry::new();
    tools
        .register(PausedTool {
            started: Arc::clone(&started),
        })
        .unwrap();
    let run_state = Arc::clone(&state);
    let run = tokio::spawn(async move {
        run_agent_loop(&run_state, "pause".to_owned(), &tools, &backend).await
    });

    tool_started.await;
    state.interrupt().unwrap();

    assert_eq!(run.await.unwrap(), Err("run interrupted".to_owned()));
    assert_eq!(
        receive_event_kinds(&mut events, 5).await,
        vec![
            SessionEventKind::RunStarted {
                input: "pause".to_owned(),
            },
            SessionEventKind::ToolRequested {
                call_id: "call-1".to_owned(),
                name: "pause".to_owned(),
                arguments: "{}".to_owned(),
            },
            SessionEventKind::ToolStarted {
                call_id: "call-1".to_owned(),
                name: "pause".to_owned(),
                arguments: "{}".to_owned(),
            },
            SessionEventKind::ToolInterrupted {
                call_id: "call-1".to_owned(),
                name: "pause".to_owned(),
            },
            SessionEventKind::RunInterrupted,
        ]
    );
    let history = state.messages.read().await;
    assert_eq!(history.len(), 3);
    assert!(matches!(
        history.last(),
        Some(Message::ToolResult(result))
            if result.call_id == "call-1"
                && result.name == "pause"
                && result.output == "run interrupted before tool completion"
                && result.is_error
    ));
}

#[tokio::test]
async fn returns_tool_failures_to_the_provider() {
    let manager = SessionManager::new();
    let state = manager
        .create(SessionOptions::new("test-provider"))
        .unwrap();
    let mut events = state.subscribe();
    let backend = FakeBackend::new([
        ProviderCompletion {
            content: vec![AssistantContent::ToolCall(ToolCall {
                id: "call-1".to_owned(),
                name: "missing".to_owned(),
                arguments: "{}".to_owned(),
            })],
        },
        ProviderCompletion {
            content: vec![AssistantContent::Text(
                "I could not run that tool.".to_owned(),
            )],
        },
    ]);

    let response = run_agent_loop(
        &state,
        "use a missing tool".to_owned(),
        &ToolRegistry::new(),
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
async fn emits_a_failed_terminal_event_when_the_provider_fails() {
    let manager = SessionManager::new();
    let state = manager
        .create(SessionOptions::new("test-provider"))
        .unwrap();
    let mut events = state.subscribe();

    let error = run_agent_loop(
        &state,
        "hello".to_owned(),
        &ToolRegistry::new(),
        &FakeBackend::new([]),
    )
    .await
    .unwrap_err();

    assert_eq!(error, "fake provider ran out of completions");
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

async fn receive_event_kinds(events: &mut SessionEvents, count: usize) -> Vec<SessionEventKind> {
    let mut kinds = Vec::with_capacity(count);
    for expected_sequence in 1..=count as u64 {
        let event = events.recv().await.unwrap().unwrap();
        assert_eq!(event.sequence, expected_sequence);
        kinds.push(event.kind);
    }
    kinds
}

struct FakeBackend {
    completions: Mutex<VecDeque<ProviderCompletion>>,
    requests: Mutex<Vec<Vec<Message>>>,
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
        let completion = self
            .completions
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| "paused provider ran out of completions".to_owned());

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
        let completion = self
            .completions
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| "fake provider ran out of completions".to_owned());
        Box::pin(async move { completion })
    }
}

struct EchoTool;

struct DropProbe {
    dropped: mpsc::SyncSender<std::thread::ThreadId>,
}

impl Drop for DropProbe {
    fn drop(&mut self) {
        self.dropped.send(std::thread::current().id()).unwrap();
    }
}

impl Tool for DropProbe {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "drop-probe".to_owned(),
            description: "Reports the thread that drops it".to_owned(),
            parameters: r#"{"type":"object"}"#.to_owned(),
        }
    }

    fn execute(
        &self,
        _arguments: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
    {
        Box::pin(async { Ok(String::new()) })
    }
}

struct PausedTool {
    started: Arc<Notify>,
}

impl Tool for PausedTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "pause".to_owned(),
            description: "Waits forever".to_owned(),
            parameters: r#"{"type":"object"}"#.to_owned(),
        }
    }

    fn execute(
        &self,
        _arguments: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
    {
        Box::pin(async move {
            self.started.notify_one();
            std::future::pending().await
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

fn provider_component(id: &str) -> Vec<u8> {
    plugin_component(id, "provider-plugin", Some(permissive_schema()))
}

fn provider_component_with_schema(id: &str, schema: &str) -> Vec<u8> {
    plugin_component(id, "provider-plugin", Some(schema))
}

fn provider_component_with_trapping_schema(id: &str) -> Vec<u8> {
    plugin_component(id, "provider-plugin", None)
}

fn tool_component(id: &str) -> Vec<u8> {
    plugin_component(id, "tool-plugin", Some(permissive_schema()))
}

fn tool_component_with_schema(id: &str, schema: &str) -> Vec<u8> {
    plugin_component(id, "tool-plugin", Some(schema))
}

fn unsupported_component(id: &str) -> Vec<u8> {
    let mut resolve = Resolve::new();
    resolve
        .push_str(
            "lockgate-config.wit",
            r#"
package lockgate:config;

interface schema {
  settings-schema: func() -> string;
}
"#,
        )
        .unwrap();
    let package = resolve
        .push_str(
            "fixture.wit",
            r#"
package sage:test;

world fixture {
  export lockgate:config/schema;
}
"#,
        )
        .unwrap();
    let world = resolve.packages[package].worlds["fixture"];
    let module = dummy_module(&resolve, world, ManglingAndAbi::Standard32);
    let mut module = module_with_schema(&module, permissive_schema(), false);
    embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8).unwrap();
    let bytes = ComponentEncoder::default()
        .module(&module)
        .unwrap()
        .encode()
        .unwrap();
    with_plugin_sections(bytes, id)
}

fn plugin_component(id: &str, world_name: &str, schema: Option<&str>) -> Vec<u8> {
    let mut resolve = Resolve::new();
    let wit = Path::new(env!("CARGO_MANIFEST_DIR")).join("../sage-plugin/wit");
    resolve.push_path(wit).unwrap();
    resolve
        .push_str(
            "lockgate-config.wit",
            r#"
package lockgate:config;

interface schema {
  settings-schema: func() -> string;
}
"#,
        )
        .unwrap();
    let wrapper = format!(
        r#"
package sage:test;

world fixture {{
  include sage:agent/{world_name}@0.1.0;
  export lockgate:config/schema;
}}
"#
    );
    let package = resolve.push_str("fixture.wit", &wrapper).unwrap();
    let world = resolve.packages[package].worlds["fixture"];
    let mut module = dummy_module(&resolve, world, ManglingAndAbi::Standard32);
    if let Some(schema) = schema {
        module = module_with_schema(&module, schema, world_name == "tool-plugin");
    }
    embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8).unwrap();
    let bytes = ComponentEncoder::default()
        .module(&module)
        .unwrap()
        .encode()
        .unwrap();
    with_plugin_sections(bytes, id)
}

fn module_with_schema(module: &[u8], schema: &str, empty_tool_definitions: bool) -> Vec<u8> {
    let definitions_result = (16 + schema.len() + 3) & !3;
    assert!(definitions_result + 12 <= 65_536);
    let mut wat = wasmprinter::print_bytes(module).unwrap();
    wat = wat.replacen("(memory (;0;) 0)", "(memory (;0;) 1)", 1);
    wat = wat.replacen(
        "(func (;0;) (type 0) (result i32)\n    unreachable\n  )",
        "(func (;0;) (type 0) (result i32)\n    i32.const 0\n  )",
        1,
    );
    if empty_tool_definitions {
        wat = wat.replacen(
            "(func (;2;) (type 0) (result i32)\n    unreachable\n  )",
            &format!("(func (;2;) (type 0) (result i32)\n    i32.const {definitions_result}\n  )"),
            1,
        );
    }
    let mut result = vec![16, 0, 0, 0];
    result.extend_from_slice(&(schema.len() as u32).to_le_bytes());
    let mut data = format!(
        "(data (i32.const 0) \"{}\")\n(data (i32.const 16) \"{}\")\n",
        wat_bytes(&result),
        wat_bytes(schema.as_bytes())
    );
    if empty_tool_definitions {
        data.push_str(&format!(
            "(data (i32.const {definitions_result}) \"{}\")\n",
            wat_bytes(&[0; 12])
        ));
    }
    data.push(')');
    wat.truncate(wat.strip_suffix(")\n").unwrap().len());
    wat.push_str(&data);
    wat::parse_str(wat).unwrap()
}

fn wat_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("\\{byte:02x}")).collect()
}

fn permissive_schema() -> &'static str {
    r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object"}"#
}

fn with_plugin_sections(bytes: Vec<u8>, id: &str) -> Vec<u8> {
    let metadata =
        format!(r#"{{"format":1,"id":"{id}","name":"Test provider","version":"0.1.0"}}"#);
    let bytes = with_custom_section(bytes, "lockgate:plugin", metadata.as_bytes());
    with_custom_section(
        bytes,
        "lockgate:needs",
        br#"{"format":1,"optional":{},"reasons":{},"required":{}}"#,
    )
}

fn with_custom_section(mut bytes: Vec<u8>, section_name: &str, contents: &[u8]) -> Vec<u8> {
    let mut section = Vec::new();
    encode_u32(section_name.len() as u32, &mut section);
    section.extend_from_slice(section_name.as_bytes());
    section.extend_from_slice(contents);
    bytes.push(0);
    encode_u32(section.len() as u32, &mut bytes);
    bytes.extend(section);
    bytes
}

fn encode_u32(mut value: u32, output: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        output.push(byte);
        if value == 0 {
            return;
        }
    }
}

fn test_directory() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "sage-{}-{}",
        std::process::id(),
        uuid::Uuid::now_v7()
    ));
    fs::create_dir(&path).unwrap();
    path
}
