use crate::config::{Config, Plugin as PluginConfig};
use crate::session::{Message, Session, SessionId, SessionManager, SessionOptions};
use crate::tool::ToolRegistry;
use crate::{Tool, ToolDefinition};
use bindings::__lockgate_world_0::exports::sage::agent::provider as provider_bindings;
use host::{AppState, HTTP_CLIENT_INTERFACE, SETTINGS_INTERFACE};
use lockgate::Component;
use std::{collections::BTreeMap, fs, path::Path};

mod bindings;
mod host;

const PLUGIN_FUEL_PER_CALL: u64 = 25_000_000;

type InnerApplication = lockgate::Application<AppState>;
type InnerRuntime = lockgate::Runtime<AppState>;
type LoadedPlugin = Component<bindings::ProviderPlugin>;

pub struct Application {
    lockgate: InnerApplication,
    plugins: BTreeMap<String, LoadedPlugin>,
    tools: ToolRegistry,
}

pub struct Runtime {
    lockgate: InnerRuntime,
    plugins: BTreeMap<String, LoadedPlugin>,
    sessions: SessionManager,
    tools: ToolRegistry,
}

impl Application {
    pub fn load(config: &Config) -> Result<Self, String> {
        let mut lockgate = lockgate::Application::new(AppState::from_config(config)?)
            .map_err(|error| format!("failed to create Lockgate application: {error}"))?
            .fuel_per_call(PLUGIN_FUEL_PER_CALL);
        let plugins = Self::load_plugins(&mut lockgate, config)?;
        let lockgate = Self::apply_policy(lockgate, &plugins)?;

        Ok(Self {
            lockgate,
            plugins,
            tools: ToolRegistry::new(),
        })
    }

    pub fn plugin_count(&self) -> usize {
        self.plugins.len()
    }

    pub fn register_tool<T>(&mut self, tool: T) -> Result<(), String>
    where
        T: Tool + 'static,
    {
        self.tools.register(tool)
    }

    pub async fn run(self) -> Result<Runtime, String> {
        let lockgate = self
            .lockgate
            .run()
            .await
            .map_err(|error| format!("failed to start plugin runtime: {error}"))?;
        Ok(Runtime {
            lockgate,
            plugins: self.plugins,
            sessions: SessionManager::new(),
            tools: self.tools,
        })
    }

    fn load_plugins(
        lockgate: &mut InnerApplication,
        config: &Config,
    ) -> Result<BTreeMap<String, LoadedPlugin>, String> {
        let mut plugins = BTreeMap::new();

        for (id, plugin) in config.plugins() {
            let loaded = Self::load_plugin(lockgate, config, id, plugin)?;
            plugins.insert(id.to_owned(), loaded);
        }

        Ok(plugins)
    }

    fn load_plugin(
        lockgate: &mut InnerApplication,
        config: &Config,
        id: &str,
        plugin: &PluginConfig,
    ) -> Result<LoadedPlugin, String> {
        let path = config.component_path(plugin);
        let bytes = fs::read(&path).map_err(|error| {
            format!(
                "failed to read plugin `{id}` from `{}`: {error}",
                path.display()
            )
        })?;
        let loaded = lockgate
            .add::<bindings::ProviderPlugin>(bytes)
            .map_err(|error| {
                format!(
                    "failed to load plugin `{id}` from `{}`: {error}",
                    path.display()
                )
            })?;

        Self::validate_plugin_id(lockgate, loaded, id, &path)?;
        Ok(loaded)
    }

    fn validate_plugin_id(
        lockgate: &InnerApplication,
        plugin: LoadedPlugin,
        id: &str,
        path: &Path,
    ) -> Result<(), String> {
        let embedded_id = lockgate
            .metadata(plugin)
            .map_err(|error| format!("failed to inspect plugin `{id}`: {error}"))?
            .id();
        if embedded_id != id {
            return Err(format!(
                "plugin `{id}` declares embedded id `{embedded_id}` in `{}`",
                path.display()
            ));
        }
        Ok(())
    }

    fn apply_policy(
        mut lockgate: InnerApplication,
        plugins: &BTreeMap<String, LoadedPlugin>,
    ) -> Result<InnerApplication, String> {
        // TODO: Move capability policy into Lockgate configuration instead of granting every
        // plugin the same host imports. HTTP should use `wasi:http`, with Lockgate granting and
        // enforcing per-plugin URL restrictions.
        for (id, plugin) in plugins {
            lockgate = lockgate
                .allow_host_import(*plugin, HTTP_CLIENT_INTERFACE)
                .map_err(|error| format!("failed to configure plugin `{id}`: {error}"))?
                .allow_host_import(*plugin, SETTINGS_INTERFACE)
                .map_err(|error| format!("failed to configure plugin `{id}`: {error}"))?;
        }

        Ok(lockgate)
    }
}

impl Runtime {
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools.definitions()
    }

    pub fn create_session(&self, options: SessionOptions) -> Result<Session, String> {
        if !self.plugins.contains_key(&options.provider) {
            return Err(format!(
                "provider plugin `{}` is not configured",
                options.provider
            ));
        }
        self.sessions.create(options)
    }

    pub fn session(&self, id: SessionId) -> Option<Session> {
        self.sessions.get(id)
    }

    pub async fn run_turn(&self, session: &Session, input: String) -> Result<String, String> {
        if !self.sessions.owns(session) {
            return Err("session does not belong to this runtime".to_owned());
        }
        if input.trim().is_empty() {
            return Err("turn input cannot be empty".to_owned());
        }

        let _turn = session.state.turn_lock.lock().await;
        session
            .state
            .messages
            .write()
            .await
            .push(Message::User(input));
        let messages = session.state.messages.read().await.clone();
        let completion = self
            .request_completion(session.provider(), messages)
            .await?;
        session
            .state
            .messages
            .write()
            .await
            .push(Message::Assistant(completion.clone()));
        Ok(completion)
    }

    async fn request_completion(
        &self,
        provider: &str,
        messages: Vec<Message>,
    ) -> Result<String, String> {
        let plugin = self
            .plugins
            .get(provider)
            .copied()
            .ok_or_else(|| format!("provider plugin `{provider}` is not configured"))?;
        let completion = self
            .lockgate
            .component(plugin)
            .complete(provider_bindings::CompletionRequest {
                messages: messages.into_iter().map(Into::into).collect(),
                // Tool definitions are wired into the request when the execution loop lands.
                tools: Vec::new(),
            })
            .await
            .map_err(|error| format!("provider plugin `{provider}` failed: {error}"))?
            .map_err(|error| format!("provider plugin `{provider}`: {error}"))?;
        completion_text(completion)
    }
}

impl From<Message> for provider_bindings::Message {
    fn from(message: Message) -> Self {
        match message {
            Message::System(content) => Self::System(content),
            Message::User(content) => Self::User(content),
            Message::Assistant(content) => {
                Self::Assistant(vec![provider_bindings::AssistantContent::Text(content)])
            }
        }
    }
}

fn completion_text(completion: provider_bindings::Completion) -> Result<String, String> {
    let mut text = Vec::new();
    for content in completion.content {
        match content {
            provider_bindings::AssistantContent::Text(content) => text.push(content),
            provider_bindings::AssistantContent::ToolCall(_) => {
                return Err(
                    "provider requested a tool, but tool execution is not enabled".to_owned(),
                );
            }
        }
    }
    if text.is_empty() {
        return Err("provider returned a completion without text".to_owned());
    }
    Ok(text.join("\n"))
}

#[cfg(test)]
mod tests;
