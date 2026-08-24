#![cfg(all(feature = "provider", feature = "tools"))]

use chap_plugin::{
    Needs, NoSettings, Plugin, Provider, Tools,
    provider::{Completion, CompletionRequest, FinishReason, ProviderError},
    tools::ToolDefinition,
};

struct Dummy;

impl Plugin for Dummy {
    const ID: &'static str = "dummy";
    const NEEDS: Needs = Needs::NOTHING;
    type Settings = NoSettings;

    fn new(_settings: Self::Settings) -> Self {
        Self
    }
}

impl Provider for Dummy {
    async fn complete(&self, _request: CompletionRequest) -> Result<Completion, ProviderError> {
        Ok(Completion {
            content: Vec::new(),
            finish_reason: FinishReason::Stop,
            usage: None,
        })
    }
}

impl Tools for Dummy {
    fn definitions(&self) -> Result<Vec<ToolDefinition>, String> {
        Ok(Vec::new())
    }

    async fn execute(&self, _name: String, _arguments: String) -> Result<String, String> {
        Ok(String::new())
    }
}

#[test]
fn role_traits_are_implementable() {
    fn assert_provider<T: Provider>() {}
    fn assert_tools<T: Tools>() {}

    assert_provider::<Dummy>();
    assert_tools::<Dummy>();
}
