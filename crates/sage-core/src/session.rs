use std::{
    collections::BTreeMap,
    fmt,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio::sync::{Mutex as AsyncMutex, RwLock};
use uuid::Uuid;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Message {
    System(String),
    User(String),
    Assistant(Vec<AssistantContent>),
    ToolResult(ToolResult),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssistantContent {
    Text(String),
    ToolCall(ToolCall),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// JSON encoded arguments supplied by the provider.
    pub arguments: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolResult {
    pub call_id: String,
    pub name: String,
    pub output: String,
    pub is_error: bool,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SessionId(Uuid);

impl fmt::Display for SessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionOptions {
    pub provider: String,
    pub system_prompt: Option<String>,
}

impl SessionOptions {
    pub fn new(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            system_prompt: None,
        }
    }

    pub fn with_system_prompt(mut self, system_prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(system_prompt.into());
        self
    }
}

#[derive(Clone)]
pub struct Session {
    pub(crate) state: Arc<SessionState>,
    executor: Arc<dyn SessionExecutor>,
}

impl Session {
    pub(crate) fn new(state: Arc<SessionState>, executor: Arc<dyn SessionExecutor>) -> Self {
        Self { state, executor }
    }

    pub fn id(&self) -> SessionId {
        self.state.id
    }

    pub fn provider(&self) -> &str {
        &self.state.provider
    }

    pub async fn history(&self) -> Vec<Message> {
        self.state.messages.read().await.clone()
    }

    pub async fn send(&self, input: impl Into<String>) -> Result<String, String> {
        self.executor.send(self, input.into()).await
    }
}

pub(crate) type SessionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;

pub(crate) trait SessionExecutor: Send + Sync {
    fn send<'a>(&'a self, session: &'a Session, input: String) -> SessionFuture<'a>;
}

pub(crate) struct SessionManager {
    sessions: Mutex<BTreeMap<SessionId, Arc<SessionState>>>,
}

impl SessionManager {
    pub(crate) fn new() -> Self {
        Self {
            sessions: Mutex::new(BTreeMap::new()),
        }
    }

    pub(crate) fn create(&self, options: SessionOptions) -> Result<Arc<SessionState>, String> {
        if options.provider.trim().is_empty() {
            return Err("a session provider is required".to_owned());
        }
        let id = SessionId(Uuid::now_v7());
        let messages = options
            .system_prompt
            .into_iter()
            .map(Message::System)
            .collect();
        let state = Arc::new(SessionState {
            id,
            provider: options.provider,
            turn_lock: AsyncMutex::new(()),
            messages: RwLock::new(messages),
        });
        self.sessions
            .lock()
            .expect("session registry lock poisoned")
            .insert(id, Arc::clone(&state));
        Ok(state)
    }

    pub(crate) fn owns(&self, state: &Arc<SessionState>) -> bool {
        self.sessions
            .lock()
            .expect("session registry lock poisoned")
            .get(&state.id)
            .is_some_and(|registered| Arc::ptr_eq(registered, state))
    }
}

pub(crate) struct SessionState {
    id: SessionId,
    provider: String,
    pub(crate) turn_lock: AsyncMutex<()>,
    pub(crate) messages: RwLock<Vec<Message>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoExecutor;

    impl SessionExecutor for EchoExecutor {
        fn send<'a>(&'a self, _session: &'a Session, input: String) -> SessionFuture<'a> {
            Box::pin(async move { Ok(input) })
        }
    }

    #[tokio::test]
    async fn sessions_retain_system_prompts_in_their_history() {
        let manager = SessionManager::new();
        let state = manager
            .create(SessionOptions::new("provider").with_system_prompt("be helpful"))
            .unwrap();

        assert_eq!(
            *state.messages.read().await,
            vec![Message::System("be helpful".to_owned())]
        );
        assert!(manager.owns(&state));
    }

    #[tokio::test]
    async fn sessions_send_through_their_executor() {
        let manager = SessionManager::new();
        let state = manager.create(SessionOptions::new("provider")).unwrap();
        let session = Session::new(state, Arc::new(EchoExecutor));

        assert_eq!(session.send("hello").await.unwrap(), "hello");
    }
}
