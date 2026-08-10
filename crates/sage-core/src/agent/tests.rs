use super::{
    AgentBuilder,
    provider::{CompletionBackend, CompletionFuture, ProviderCompletion},
    turn::run_agent_loop,
};
use crate::{
    AssistantContent, Message, SessionOptions, Tool, ToolCall, ToolDefinition,
    session::SessionManager, tool::ToolRegistry,
};
use std::{collections::VecDeque, fs, path::Path, sync::Mutex, time::SystemTime};
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
    let backend = FakeBackend::new([
        ProviderCompletion {
            content: vec![AssistantContent::ToolCall(ToolCall {
                id: "call-1".to_owned(),
                name: "echo".to_owned(),
                arguments: r#"{"message":"hello"}"#.to_owned(),
            })],
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
            if matches!(content.as_slice(), [AssistantContent::ToolCall(call)] if call.name == "echo")
    ));
    assert!(matches!(history[2], Message::ToolResult(_)));
    assert!(matches!(
        history[3],
        Message::Assistant(ref content)
            if matches!(content.as_slice(), [AssistantContent::Text(text)] if text == "The tool said hello.")
    ));
}

#[tokio::test]
async fn returns_tool_failures_to_the_provider() {
    let manager = SessionManager::new();
    let state = manager
        .create(SessionOptions::new("test-provider"))
        .unwrap();
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
    let requests = backend.requests.lock().unwrap();
    assert!(matches!(
        requests[1].last(),
        Some(Message::ToolResult(result))
            if result.output == "tool `missing` is not registered" && result.is_error
    ));
}

struct FakeBackend {
    completions: Mutex<VecDeque<ProviderCompletion>>,
    requests: Mutex<Vec<Vec<Message>>>,
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
