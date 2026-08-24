use chap_plugin::provider::{
    AssistantContent, Completion, CompletionRequest, FinishReason, Message, Provider, ProviderError,
};
use chap_plugin::{MetadataSource, Needs, NoSettings, Plugin};

struct SingleRole;

impl Plugin for SingleRole {
    const ID: &'static str = "single-role";
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

impl Provider for SingleRole {
    async fn complete(&self, request: CompletionRequest) -> Result<Completion, ProviderError> {
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

chap_plugin::plugin!(SingleRole: Provider);

#[test]
fn plugin_macro_supports_single_role_tests() {}
