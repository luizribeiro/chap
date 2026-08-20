use sage::provider::{AssistantContent, Completion, CompletionRequest, FinishReason, Message};
use sage::tools::ToolDefinition;
use sage::{MetadataSource, Needs, NoSettings, Plugin, Provider, Tools};
use sage_plugin as sage;

struct MultiRole;

impl Plugin for MultiRole {
    const ID: &'static str = "multi-role";
    const DESCRIPTION: MetadataSource = MetadataSource::Absent;
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::NOTHING;
    type Settings = NoSettings;

    fn new(_settings: Self::Settings) -> Self {
        Self
    }
}

impl Provider for MultiRole {
    async fn complete(&self, request: CompletionRequest) -> Result<Completion, String> {
        Ok(Completion {
            content: request
                .messages
                .into_iter()
                .filter_map(|message| match message {
                    Message::User(text) => Some(AssistantContent::Text(text)),
                    _ => None,
                })
                .collect(),
            finish_reason: FinishReason::Stop,
        })
    }
}

impl Tools for MultiRole {
    fn definitions(&self) -> Result<Vec<ToolDefinition>, String> {
        Ok(vec![ToolDefinition {
            name: "echo".to_owned(),
            description: "Returns its arguments".to_owned(),
            parameters: r#"{"type":"object"}"#.to_owned(),
        }])
    }

    async fn execute(&self, name: String, arguments: String) -> Result<String, String> {
        match name.as_str() {
            "echo" => Ok(arguments),
            _ => Err(format!("unknown tool `{name}`")),
        }
    }
}

sage::plugin!(MultiRole: Provider + Tools);

#[test]
fn generated_world_exports_both_roles() {}
