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
use std::{
    collections::VecDeque,
    fs,
    path::Path,
    sync::{Arc, Mutex},
    time::SystemTime,
};
use tokio::sync::Notify;
use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
use wit_parser::{ManglingAndAbi, Resolve};

#[tokio::test]
async fn loads_a_configured_provider() {
    let directory = test_directory();
    let component = directory.join("provider.wasm");
    fs::write(&component, provider_component("example.provider")).unwrap();
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
    builder.start().await.unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn rejects_a_config_id_that_differs_from_plugin_metadata() {
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

    let error = match AgentBuilder::load(&config_path).unwrap().start().await {
        Ok(_) => panic!("mismatched plugin id should be rejected"),
        Err(error) => error,
    };

    assert!(error.contains("plugin `config-id` declares embedded id `embedded.id`"));
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
    let mut resolve = Resolve::new();
    let wit = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../wit");
    let package = resolve.push_path(wit).unwrap().0;
    let world = resolve.packages[package].worlds["provider-plugin"];
    let mut module = dummy_module(&resolve, world, ManglingAndAbi::Standard32);
    embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8).unwrap();
    let bytes = ComponentEncoder::default()
        .module(&module)
        .unwrap()
        .encode()
        .unwrap();
    with_plugin_metadata(bytes, id)
}

fn with_plugin_metadata(mut bytes: Vec<u8>, id: &str) -> Vec<u8> {
    let metadata =
        format!(r#"{{"format":1,"id":"{id}","name":"Test provider","version":"0.1.0"}}"#);
    let section_name = "lockgate:plugin";
    let mut section = Vec::new();
    encode_u32(section_name.len() as u32, &mut section);
    section.extend_from_slice(section_name.as_bytes());
    section.extend_from_slice(metadata.as_bytes());
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
    let unique = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("sage-{}-{unique}", std::process::id()));
    fs::create_dir(&path).unwrap();
    path
}
