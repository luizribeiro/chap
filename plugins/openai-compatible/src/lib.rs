use chap::provider::{Completion, CompletionRequest};
use chap_plugin as chap;
use chap_plugin::{MetadataSource, Needs, Plugin, Provider, ScopeRef, env, net};

mod chat_completions;

struct OpenAiCompatible {
    settings: Settings,
}

chap::plugin!(OpenAiCompatible: Provider);

impl Plugin for OpenAiCompatible {
    const ID: &'static str = "openai";
    const DISPLAY_NAME: MetadataSource = MetadataSource::Explicit("OpenAI-compatible provider");
    const DESCRIPTION: MetadataSource =
        MetadataSource::Explicit("Calls an OpenAI-compatible Chat Completions server");
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::required(&[net::EGRESS.need(&[ScopeRef::setting("/base-url")])])
        .optional(&[env::READ.need(&[ScopeRef::setting("/api-key-env")])]);
    type Settings = Settings;

    fn new(settings: Self::Settings) -> Self {
        Self { settings }
    }
}

#[derive(chap::Settings)]
struct Settings {
    base_url: String,
    model: String,
    api_key_env: Option<String>,
}

impl Provider for OpenAiCompatible {
    async fn complete(&self, request: CompletionRequest) -> Result<Completion, String> {
        let settings = &self.settings;
        let request = chat_completions::encode_request(&settings.model, request)?;
        let url = format!(
            "{}/chat/completions",
            settings.base_url.trim_end_matches('/')
        );
        let api_key = settings
            .api_key_env
            .as_deref()
            .map(read_environment_variable)
            .transpose()?;
        let response = chap::http::Client::new()
            .post(&url)
            .header("content-type", "application/json")
            .map_err(|error| error.to_string())?
            .bearer(api_key.as_deref().filter(|api_key| !api_key.is_empty()))
            .body(request.into_bytes())
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let status = response.status();
        let body = response.text().map_err(|error| error.to_string())?;
        chat_completions::parse_response(status, &body)
    }
}

fn read_environment_variable(name: &str) -> Result<String, String> {
    std::env::var(name)
        .map_err(|_| format!("environment variable `{name}` is not available to the plugin"))
}
