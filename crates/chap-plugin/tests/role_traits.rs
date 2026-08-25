use chap_plugin::{
    Needs, NoSettings, Plugin,
    context::{Context, Segment},
    provider::{Completion, CompletionRequest, FinishReason, Provider, ProviderError},
    tools::{ToolDefinition, Tools},
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

impl Context for Dummy {
    async fn segments(&self) -> Result<Vec<Segment>, String> {
        Ok(Vec::new())
    }
}

#[test]
fn role_traits_are_implementable() {
    fn assert_provider<T: Provider>() {}
    fn assert_tools<T: Tools>() {}
    fn assert_context<T: Context>() {}

    assert_provider::<Dummy>();
    assert_tools::<Dummy>();
    assert_context::<Dummy>();
}
