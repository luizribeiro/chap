use std::{
    collections::BTreeMap,
    fmt,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio::sync::{Mutex as AsyncMutex, RwLock, broadcast};
use uuid::Uuid;

const EVENT_CHANNEL_CAPACITY: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Message {
    System(String),
    User(String),
    Assistant(Vec<AssistantContent>),
    ToolResult(ToolResult),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AssistantContent {
    Text(String),
    ToolCall(ToolCall),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ToolCall {
    pub(crate) id: String,
    pub(crate) name: String,
    /// JSON encoded arguments supplied by the provider.
    pub(crate) arguments: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ToolResult {
    pub(crate) call_id: String,
    pub(crate) name: String,
    pub(crate) output: String,
    pub(crate) is_error: bool,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SessionId(Uuid);

impl fmt::Display for SessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct SessionEvent {
    pub sequence: u64,
    pub kind: SessionEventKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SessionEventKind {
    RunStarted {
        input: String,
    },
    AssistantMessage {
        text: String,
    },
    ToolRequested {
        call_id: String,
        name: String,
        arguments: String,
    },
    ToolStarted {
        call_id: String,
        name: String,
        arguments: String,
    },
    ToolFinished {
        call_id: String,
        name: String,
        result: Result<String, String>,
    },
    RunCompleted {
        response: String,
    },
    RunFailed {
        error: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SessionEventError {
    Lagged { skipped: u64 },
}

impl fmt::Display for SessionEventError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lagged { skipped } => {
                write!(formatter, "session observer skipped {skipped} events")
            }
        }
    }
}

impl std::error::Error for SessionEventError {}

pub struct SessionEvents {
    receiver: broadcast::Receiver<SessionEvent>,
}

impl SessionEvents {
    pub async fn recv(&mut self) -> Result<Option<SessionEvent>, SessionEventError> {
        match self.receiver.recv().await {
            Ok(event) => Ok(Some(event)),
            Err(broadcast::error::RecvError::Closed) => Ok(None),
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                Err(SessionEventError::Lagged { skipped })
            }
        }
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

    pub fn subscribe(&self) -> SessionEvents {
        SessionEvents {
            receiver: self.state.events.subscribe(),
        }
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
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let state = Arc::new(SessionState {
            id,
            provider: options.provider,
            turn_lock: AsyncMutex::new(()),
            messages: RwLock::new(messages),
            events,
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
    events: broadcast::Sender<SessionEvent>,
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

    #[tokio::test]
    async fn session_subscribers_observe_events_independently_and_in_order() {
        let manager = SessionManager::new();
        let state = manager.create(SessionOptions::new("provider")).unwrap();
        let session = Session::new(Arc::clone(&state), Arc::new(EchoExecutor));
        let mut first = session.subscribe();
        let mut second = session.subscribe();
        let started = SessionEvent {
            sequence: 1,
            kind: SessionEventKind::RunStarted {
                input: "hello".to_owned(),
            },
        };
        let completed = SessionEvent {
            sequence: 2,
            kind: SessionEventKind::RunCompleted {
                response: "hi".to_owned(),
            },
        };

        state.events.send(started.clone()).unwrap();
        state.events.send(completed.clone()).unwrap();

        assert_eq!(first.recv().await.unwrap(), Some(started.clone()));
        assert_eq!(first.recv().await.unwrap(), Some(completed.clone()));
        assert_eq!(second.recv().await.unwrap(), Some(started));
        assert_eq!(second.recv().await.unwrap(), Some(completed));
    }

    #[tokio::test]
    async fn session_subscribers_report_lag() {
        let manager = SessionManager::new();
        let state = manager.create(SessionOptions::new("provider")).unwrap();
        let session = Session::new(Arc::clone(&state), Arc::new(EchoExecutor));
        let mut events = session.subscribe();

        for sequence in 1..=(EVENT_CHANNEL_CAPACITY as u64 + 1) {
            state
                .events
                .send(SessionEvent {
                    sequence,
                    kind: SessionEventKind::AssistantMessage {
                        text: sequence.to_string(),
                    },
                })
                .unwrap();
        }

        assert_eq!(
            events.recv().await,
            Err(SessionEventError::Lagged { skipped: 1 })
        );
    }
}
