#![cfg(all(feature = "provider", feature = "tools"))]

use sage_plugin::{
    Provider, Tools,
    provider::{Completion, CompletionRequest, FinishReason},
    tools::ToolDefinition,
};

struct Dummy;

impl Provider for Dummy {
    async fn complete(&self, _request: CompletionRequest) -> Result<Completion, String> {
        Ok(Completion {
            content: Vec::new(),
            finish_reason: FinishReason::Stop,
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
