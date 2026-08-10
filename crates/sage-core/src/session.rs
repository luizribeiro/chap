use std::{
    collections::BTreeMap,
    fmt,
    sync::{Arc, Mutex},
};
use tokio::sync::{Mutex as AsyncMutex, RwLock};
use uuid::Uuid;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Message {
    System(String),
    User(String),
    Assistant(String),
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
}

impl Session {
    pub fn id(&self) -> SessionId {
        self.state.id
    }

    pub fn provider(&self) -> &str {
        &self.state.provider
    }

    pub async fn history(&self) -> Vec<Message> {
        self.state.messages.read().await.clone()
    }
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

    pub(crate) fn create(&self, options: SessionOptions) -> Result<Session, String> {
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
        Ok(Session { state })
    }

    pub(crate) fn get(&self, id: SessionId) -> Option<Session> {
        self.sessions
            .lock()
            .expect("session registry lock poisoned")
            .get(&id)
            .cloned()
            .map(|state| Session { state })
    }

    pub(crate) fn owns(&self, session: &Session) -> bool {
        self.sessions
            .lock()
            .expect("session registry lock poisoned")
            .get(&session.id())
            .is_some_and(|state| Arc::ptr_eq(state, &session.state))
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

    #[tokio::test]
    async fn sessions_retain_system_prompts_in_their_history() {
        let manager = SessionManager::new();
        let session = manager
            .create(SessionOptions::new("provider").with_system_prompt("be helpful"))
            .unwrap();

        assert_eq!(
            session.history().await,
            vec![Message::System("be helpful".to_owned())]
        );
        assert!(manager.owns(&session));
        assert_eq!(manager.get(session.id()).unwrap().id(), session.id());
    }
}
