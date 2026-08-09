mod bindings {
    lockgate_plugin::bindings!({
        path: "../../wit",
        world: "provider-plugin",
        metadata: {
            id: "openai",
            name: "OpenAI-compatible provider",
            version: "0.1.0",
            description: "Placeholder for an OpenAI-compatible model provider",
        },
    });
}

use bindings::exports::sage::agent::provider::Guest;
use bindings::sage::agent::settings;

struct OpenAiCompatible;

impl Guest for OpenAiCompatible {
    fn complete(_prompt: String) -> Result<String, String> {
        let model = settings::get("model").unwrap_or_else(|| "unconfigured".to_owned());
        Err(format!(
            "OpenAI-compatible provider for model `{model}` is not implemented yet"
        ))
    }
}

bindings::export!(OpenAiCompatible with_types_in bindings);
