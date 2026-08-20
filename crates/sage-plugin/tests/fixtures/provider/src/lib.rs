use sage::provider::{AssistantContent, Completion, CompletionRequest, FinishReason, Message};
use sage::{MetadataSource, Needs, Plugin, Provider};
use sage_plugin as sage;

#[derive(sage::Settings)]
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
        })
    }
}

sage::plugin!(ProviderFixture: Provider);
