use chap_plugin as sage;
use sage::provider::{
    AssistantContent, Completion, CompletionRequest, FinishReason, Message, ToolCall,
};
use sage::tools::ToolDefinition;
use sage::{MetadataSource, Needs, Plugin, Provider, Tools};

#[derive(sage::Settings)]
struct Settings {
    prefix: String,
}

struct MultiRoleFixture {
    prefix: String,
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
        }
    }
}

impl Provider for MultiRoleFixture {
    async fn complete(&self, request: CompletionRequest) -> Result<Completion, String> {
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
            });
        }

        Ok(Completion {
            content: vec![AssistantContent::ToolCall(ToolCall {
                id: "fixture-call".to_owned(),
                name: "sdk-echo".to_owned(),
                arguments: r#"{"value":"round-trip"}"#.to_owned(),
            })],
            finish_reason: FinishReason::ToolCalls,
        })
    }
}

impl Tools for MultiRoleFixture {
    fn definitions(&self) -> Result<Vec<ToolDefinition>, String> {
        Ok(vec![ToolDefinition {
            name: "sdk-echo".to_owned(),
            description: "Echoes its JSON arguments".to_owned(),
            parameters:
                r#"{"type":"object","properties":{"value":{"type":"string"}},"required":["value"]}"#
                    .to_owned(),
        }])
    }

    async fn execute(&self, name: String, arguments: String) -> Result<String, String> {
        match name.as_str() {
            "sdk-echo" => Ok(format!("tool:{arguments}")),
            _ => Err(format!("unknown tool `{name}`")),
        }
    }
}

sage::plugin!(MultiRoleFixture: Provider + Tools);
