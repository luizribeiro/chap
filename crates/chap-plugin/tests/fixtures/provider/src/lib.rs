use chap::provider::{AssistantContent, Completion, CompletionRequest, FinishReason, Message};
use chap::{MetadataSource, Needs, Plugin, Provider};
use chap_plugin as chap;

#[derive(chap::Settings)]
struct Settings {
    prefix: String,
}

struct ProviderFixture {
    prefix: String,
}

impl Plugin for ProviderFixture {
    const ID: &'static str = "sdk-provider-fixture";
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

impl Provider for ProviderFixture {
    async fn complete(&self, request: CompletionRequest) -> Result<Completion, String> {
        let input = request
            .messages
            .into_iter()
            .rev()
            .find_map(|message| match message {
                Message::User(input) => Some(input),
                _ => None,
            })
            .ok_or_else(|| "provider fixture expected a user message".to_owned())?;
        Ok(Completion {
            content: vec![AssistantContent::Text(format!("{}{}", self.prefix, input))],
            finish_reason: FinishReason::Stop,
            usage: None,
        })
    }
}

chap::plugin!(ProviderFixture: Provider);
