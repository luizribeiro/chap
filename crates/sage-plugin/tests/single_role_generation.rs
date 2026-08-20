use sage::provider::{AssistantContent, Completion, CompletionRequest, FinishReason, Message};
use sage::{Needs, NoSettings, Plugin, Provider};
use sage_plugin as sage;

struct SingleRole;

impl Plugin for SingleRole {
    const ID: &'static str = "single-role";
    const NEEDS: Needs = Needs::NOTHING;
    type Settings = NoSettings;

    fn new(_settings: Self::Settings) -> Self {
        Self
    }
}

impl Provider for SingleRole {
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

sage::plugin!(SingleRole: Provider);

impl exports::sage::agent::provider::Guest for SingleRole {
    async fn complete(request: CompletionRequest) -> Result<Completion, String> {
        <Self as Provider>::complete(&Self, request).await
    }
}

#[test]
fn generated_guest_reuses_sdk_types() {}
