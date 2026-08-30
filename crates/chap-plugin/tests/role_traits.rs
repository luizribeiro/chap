use chap_plugin::{
    Needs, NoSettings, Plugin,
    context::{Context, Segment},
    provider::{Completion, CompletionRequest, FinishReason, Provider, ProviderError},
    tools::{ToolDefinition, ToolError, Tools},
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

    async fn execute(&self, _name: String, _arguments: String) -> Result<String, ToolError> {
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

#[test]
fn tool_error_preserves_each_failure_category() {
    let errors = [
        ToolError::InvalidInput("invalid".to_owned()),
        ToolError::Denied("denied".to_owned()),
        ToolError::Failed("failed".to_owned()),
        ToolError::Fatal("fatal".to_owned()),
    ];

    assert!(matches!(&errors[0], ToolError::InvalidInput(message) if message == "invalid"));
    assert!(matches!(&errors[1], ToolError::Denied(message) if message == "denied"));
    assert!(matches!(&errors[2], ToolError::Failed(message) if message == "failed"));
    assert!(matches!(&errors[3], ToolError::Fatal(message) if message == "fatal"));
}
