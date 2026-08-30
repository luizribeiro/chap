use chap_plugin::context::{Context, Segment};
use chap_plugin::provider::{
    AssistantContent, Completion, CompletionRequest, FinishReason, Message, Provider,
    ProviderError, ToolCall,
};
use chap_plugin::tools::{ToolDefinition, ToolError, Tools};
use chap_plugin::{MetadataSource, Needs, Plugin};
use schemars::JsonSchema;
use serde::Deserialize;
use std::time::Duration;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Settings {
    #[schemars(regex(pattern = r"\S"))]
    prefix: String,
    #[serde(default)]
    hang_definitions: bool,
    #[serde(default)]
    fail_definitions: bool,
}

struct MultiRoleFixture {
    prefix: String,
    hang_definitions: bool,
    fail_definitions: bool,
}

impl Plugin for MultiRoleFixture {
    const ID: &'static str = "sdk-multi-role-fixture";
    const DESCRIPTION: MetadataSource = MetadataSource::Absent;
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::NOTHING;
    type Settings = Settings;

    fn new(settings: Self::Settings) -> Self {
        Self {
            prefix: settings.prefix,
            hang_definitions: settings.hang_definitions,
            fail_definitions: settings.fail_definitions,
        }
    }
}

impl Provider for MultiRoleFixture {
    async fn complete(&self, request: CompletionRequest) -> Result<Completion, ProviderError> {
        if let Some(output) = request
            .messages
            .into_iter()
            .rev()
            .find_map(|message| match message {
                Message::ToolResult(result) => Some(result.output),
                _ => None,
            })
        {
            return Ok(Completion {
                content: vec![AssistantContent::Text(format!("{}{}", self.prefix, output))],
                finish_reason: FinishReason::Stop,
                usage: None,
            });
        }

        Ok(Completion {
            content: vec![AssistantContent::ToolCall(ToolCall {
                id: "fixture-call".to_owned(),
                name: "sdk-echo".to_owned(),
                arguments: r#"{"value":"round-trip"}"#.to_owned(),
            })],
            finish_reason: FinishReason::ToolCalls,
            usage: None,
        })
    }
}

impl Tools for MultiRoleFixture {
    fn definitions(&self) -> Result<Vec<ToolDefinition>, String> {
        if self.hang_definitions {
            std::thread::sleep(Duration::from_secs(60));
        }
        if self.fail_definitions {
            return Err("tool catalog is unavailable".to_owned());
        }
        Ok(vec![ToolDefinition {
            name: "sdk-echo".to_owned(),
            description: "Echoes its JSON arguments".to_owned(),
            parameters:
                r#"{"type":"object","properties":{"value":{"type":"string"}},"required":["value"]}"#
                    .to_owned(),
        }])
    }

    async fn execute(&self, name: String, arguments: String) -> Result<String, ToolError> {
        match name.as_str() {
            "sdk-echo" => Ok(format!("tool:{arguments}")),
            "sdk-invalid-input" => Err(ToolError::InvalidInput(
                "arguments did not match the tool schema".to_owned(),
            )),
            "sdk-denied" => Err(ToolError::Denied(
                "capability policy denied the request".to_owned(),
            )),
            "sdk-failed" => Err(ToolError::Failed(
                "remote tool execution returned an error".to_owned(),
            )),
            "sdk-fatal" => Err(ToolError::Fatal(
                "plugin runtime configuration is unavailable".to_owned(),
            )),
            _ => Err(ToolError::InvalidInput(format!("unknown tool `{name}`"))),
        }
    }
}

impl Context for MultiRoleFixture {
    async fn segments(&self) -> Result<Vec<Segment>, String> {
        Ok(vec![Segment {
            id: "sdk-context".to_owned(),
            content: format!("{}context", self.prefix),
            priority: 7,
        }])
    }
}

chap_plugin::plugin!(MultiRoleFixture: Provider + Tools + Context);
