use chap::provider::{AssistantContent, Completion, CompletionRequest, FinishReason, Message};
use chap::tools::ToolDefinition;
use chap::{MetadataSource, Needs, NoSettings, Plugin, Provider, Tools};
use chap_plugin as chap;

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
            usage: None,
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

chap::plugin!(MultiRole: Provider + Tools);

#[test]
fn plugin_macro_supports_multi_role_tests() {}
