use crate::{FinishReason, ProviderError, ToolError};
use std::{
    collections::{BTreeMap, VecDeque},
    fmt,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};
use thiserror::Error;
use tokio::sync::{Mutex as AsyncMutex, RwLock, broadcast, watch};
use uuid::Uuid;

const EVENT_CHANNEL_CAPACITY: usize = 256;

/// Context-plugin output assembled once at session creation and prepended
/// to the history clone sent with every provider request.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct AssembledContext {
    pub(crate) system: Option<String>,
    pub(crate) context: Option<String>,
}

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
    Reasoning(Reasoning),
    ToolCall(ToolCall),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Reasoning {
    pub(crate) text: String,
    pub(crate) signature: Option<String>,
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Usage {
    pub input_tokens: u64,
    pub cached_input_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub output_tokens: u64,
    pub reasoning_tokens: Option<u64>,
}

impl Usage {
    /// Saturating-adds another measurement into this usage total. Optional
    /// subset totals stay absent until a measurement reports them, so an
    /// unreported counter remains distinct from a reported zero.
    pub fn saturating_add_assign(&mut self, usage: Self) {
        self.input_tokens = self.input_tokens.saturating_add(usage.input_tokens);
        saturating_add_optional_assign(&mut self.cached_input_tokens, usage.cached_input_tokens);
        saturating_add_optional_assign(&mut self.cache_write_tokens, usage.cache_write_tokens);
        self.output_tokens = self.output_tokens.saturating_add(usage.output_tokens);
        saturating_add_optional_assign(&mut self.reasoning_tokens, usage.reasoning_tokens);
    }
}

fn saturating_add_optional_assign(total: &mut Option<u64>, value: Option<u64>) {
    if let Some(value) = value {
        *total = Some(total.unwrap_or_default().saturating_add(value));
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RunUsage {
    /// Accumulated across every provider step of the run so far.
    pub total: Usage,
    /// The latest provider step alone.
    pub last_step: Usage,
}

#[derive(Clone, Debug, Error)]
#[non_exhaustive]
pub enum RunError {
    #[error(transparent)]
    Provider(ProviderError),
    #[error("session run task failed: {source}")]
    SessionTask {
        #[source]
        source: Arc<tokio::task::JoinError>,
    },
    #[error("session does not belong to this runtime")]
    ForeignSession,
    #[error("turn input cannot be empty")]
    EmptyInput,
    #[error("run interrupted")]
    Interrupted,
    #[error("{}", completion_without_text_message(.finish_reason))]
    CompletionWithoutText { finish_reason: FinishReason },
    #[error("turn exceeded the limit of {limit} provider requests")]
    ProviderStepLimitExceeded { limit: usize },
}

impl PartialEq for RunError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Provider(left), Self::Provider(right)) => left == right,
            (Self::SessionTask { source: left }, Self::SessionTask { source: right }) => {
                Arc::ptr_eq(left, right)
            }
            (Self::ForeignSession, Self::ForeignSession) => true,
            (Self::EmptyInput, Self::EmptyInput) | (Self::Interrupted, Self::Interrupted) => true,
            (
                Self::CompletionWithoutText {
                    finish_reason: left,
                },
                Self::CompletionWithoutText {
                    finish_reason: right,
                },
            ) => left == right,
            (
                Self::ProviderStepLimitExceeded { limit: left },
                Self::ProviderStepLimitExceeded { limit: right },
            ) => left == right,
            _ => false,
        }
    }
}

impl Eq for RunError {}

fn completion_without_text_message(finish_reason: &FinishReason) -> String {
    match finish_reason {
        FinishReason::Length => {
            "model reached its output limit before producing a response".to_owned()
        }
        FinishReason::Other(reason) => {
            format!("provider returned a completion without text (finish reason: {reason})")
        }
        FinishReason::Stop | FinishReason::ToolCalls => {
            "provider returned a completion without text".to_owned()
        }
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("context plugin `{plugin}` failed: {error}")]
pub struct ContextFailure {
    pub plugin: String,
    pub error: String,
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SessionError {
    #[error("a session provider is required")]
    ProviderRequired,
    #[error("provider plugin `{provider}` is not configured")]
    ProviderNotConfigured { provider: String },
    #[error("{} context plugin(s) failed", .0.len())]
    Context(Vec<ContextFailure>),
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum SteerError {
    #[error("steering input cannot be empty")]
    EmptyInput,
    #[error("session has no active run to steer")]
    NoActiveRunToSteer,
    #[error("session run is being interrupted")]
    RunBeingInterrupted,
    #[error("steering input `{id}` is not queued")]
    NotQueued { id: SteeringId },
    #[error("session has no active run to interrupt")]
    NoActiveRunToInterrupt,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SessionId(Uuid);

impl fmt::Display for SessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SteeringId(Uuid);

impl fmt::Display for SteeringId {
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
    SteeringQueued {
        id: SteeringId,
        input: String,
    },
    SteeringApplied {
        id: SteeringId,
        input: String,
    },
    SteeringDiscarded {
        id: SteeringId,
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
        result: Result<String, ToolError>,
    },
    ToolInterrupted {
        call_id: String,
        name: String,
    },
    UsageUpdated {
        usage: RunUsage,
    },
    RunCompleted {
        response: String,
    },
    RunFailed {
        error: RunError,
    },
    RunInterrupted,
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
}

impl SessionOptions {
    pub fn new(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
        }
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
        self.state.subscribe()
    }

    pub async fn send(&self, input: impl Into<String>) -> Result<String, RunError> {
        let session = self.clone();
        let executor = Arc::clone(&self.executor);
        let input = input.into();
        tokio::spawn(async move { executor.send(&session, input).await })
            .await
            .map_err(|source| RunError::SessionTask {
                source: Arc::new(source),
            })?
    }

    pub fn steer(&self, input: impl Into<String>) -> Result<SteeringId, SteerError> {
        let input = input.into();
        if input.trim().is_empty() {
            return Err(SteerError::EmptyInput);
        }
        self.state.steer(input)
    }

    pub fn discard_steering(&self, id: SteeringId) -> Result<(), SteerError> {
        self.state.discard_steering(id)
    }

    pub fn interrupt(&self) -> Result<(), SteerError> {
        self.state.interrupt()
    }
}

pub(crate) type SessionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<String, RunError>> + Send + 'a>>;

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

    pub(crate) fn create(
        &self,
        options: SessionOptions,
        assembled_context: AssembledContext,
    ) -> Result<Arc<SessionState>, SessionError> {
        if options.provider.trim().is_empty() {
            return Err(SessionError::ProviderRequired);
        }
        let id = SessionId(Uuid::now_v7());
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let state = Arc::new(SessionState {
            id,
            provider: options.provider,
            assembled_context,
            turn_lock: AsyncMutex::new(()),
            run: Mutex::new(RunState::default()),
            messages: RwLock::new(Vec::new()),
            next_event_sequence: Mutex::new(1),
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
    pub(crate) assembled_context: AssembledContext,
    pub(crate) turn_lock: AsyncMutex<()>,
    run: Mutex<RunState>,
    pub(crate) messages: RwLock<Vec<Message>>,
    next_event_sequence: Mutex<u64>,
    events: broadcast::Sender<SessionEvent>,
}

#[derive(Default)]
struct RunState {
    active: bool,
    interrupted: bool,
    interrupt: Option<watch::Sender<()>>,
    steering: VecDeque<Steering>,
}

#[derive(Clone)]
pub(crate) struct Steering {
    pub(crate) id: SteeringId,
    pub(crate) input: String,
}

pub(crate) enum RunBoundary {
    ApplySteering(Vec<Steering>),
    Complete,
    Interrupted,
}

impl SessionState {
    pub(crate) fn subscribe(&self) -> SessionEvents {
        SessionEvents {
            receiver: self.events.subscribe(),
        }
    }

    pub(crate) fn emit(&self, kind: SessionEventKind) {
        let mut sequence = self
            .next_event_sequence
            .lock()
            .expect("session event sequence lock poisoned");
        let event = SessionEvent {
            sequence: *sequence,
            kind,
        };
        *sequence = sequence
            .checked_add(1)
            .expect("session event sequence exhausted");
        let _ = self.events.send(event);
    }

    pub(crate) fn start_run(&self) -> ActiveRun<'_> {
        let mut run = self.run.lock().expect("session run state lock poisoned");
        debug_assert!(!run.active, "session run should not already be active");
        debug_assert!(
            run.steering.is_empty(),
            "inactive session should not retain queued steering"
        );
        debug_assert!(
            run.interrupt.is_none(),
            "inactive session should not retain an interrupt signal"
        );
        let (interrupt, receiver) = watch::channel(());
        run.active = true;
        run.interrupted = false;
        run.interrupt = Some(interrupt);
        ActiveRun {
            session: self,
            interrupt: receiver,
        }
    }

    pub(crate) fn interrupt(&self) -> Result<(), SteerError> {
        let mut run = self.run.lock().expect("session run state lock poisoned");
        if !run.active {
            return Err(SteerError::NoActiveRunToInterrupt);
        }
        if !run.interrupted {
            run.interrupted = true;
            run.interrupt
                .as_ref()
                .expect("active session should have an interrupt signal")
                .send_replace(());
        }
        Ok(())
    }

    pub(crate) fn steer(&self, input: String) -> Result<SteeringId, SteerError> {
        let mut run = self.run.lock().expect("session run state lock poisoned");
        if !run.active {
            return Err(SteerError::NoActiveRunToSteer);
        }
        if run.interrupted {
            return Err(SteerError::RunBeingInterrupted);
        }
        let id = SteeringId(Uuid::now_v7());
        run.steering.push_back(Steering {
            id,
            input: input.clone(),
        });
        self.emit(SessionEventKind::SteeringQueued { id, input });
        Ok(id)
    }

    fn discard_steering(&self, id: SteeringId) -> Result<(), SteerError> {
        let mut run = self.run.lock().expect("session run state lock poisoned");
        let index = run
            .steering
            .iter()
            .position(|steering| steering.id == id)
            .ok_or(SteerError::NotQueued { id })?;
        let steering = run
            .steering
            .remove(index)
            .expect("queued steering index should exist");
        self.emit(SessionEventKind::SteeringDiscarded {
            id: steering.id,
            input: steering.input,
        });
        Ok(())
    }

    pub(crate) fn take_steering(&self) -> Option<Vec<Steering>> {
        let mut run = self.run.lock().expect("session run state lock poisoned");
        if run.interrupted {
            None
        } else {
            Some(run.steering.drain(..).collect())
        }
    }

    pub(crate) fn finish_or_take_steering(&self) -> RunBoundary {
        let mut run = self.run.lock().expect("session run state lock poisoned");
        if run.interrupted {
            RunBoundary::Interrupted
        } else if run.steering.is_empty() {
            run.active = false;
            RunBoundary::Complete
        } else {
            RunBoundary::ApplySteering(run.steering.drain(..).collect())
        }
    }

    fn finish_run(&self) {
        let discarded = {
            let mut run = self.run.lock().expect("session run state lock poisoned");
            run.active = false;
            run.interrupted = false;
            run.interrupt = None;
            run.steering.drain(..).collect::<Vec<_>>()
        };
        for steering in discarded {
            self.emit(SessionEventKind::SteeringDiscarded {
                id: steering.id,
                input: steering.input,
            });
        }
    }
}

pub(crate) struct ActiveRun<'a> {
    session: &'a SessionState,
    interrupt: watch::Receiver<()>,
}

impl ActiveRun<'_> {
    pub(crate) async fn interrupted(&mut self) {
        let _ = self.interrupt.changed().await;
    }
}

impl Drop for ActiveRun<'_> {
    fn drop(&mut self) {
        self.session.finish_run();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::{sync::Notify, time::timeout};

    struct EchoExecutor;

    impl SessionExecutor for EchoExecutor {
        fn send<'a>(&'a self, _session: &'a Session, input: String) -> SessionFuture<'a> {
            Box::pin(async move { Ok(input) })
        }
    }

    struct PanicExecutor;

    impl SessionExecutor for PanicExecutor {
        fn send<'a>(&'a self, _session: &'a Session, _input: String) -> SessionFuture<'a> {
            Box::pin(async move { panic!("executor failed") })
        }
    }

    #[test]
    fn run_error_implements_std_error() {
        fn assert_error(_: &dyn std::error::Error) {}

        assert_error(&RunError::Interrupted);
    }

    #[test]
    fn session_error_implements_std_error() {
        fn assert_error(_: &dyn std::error::Error) {}

        assert_error(&SessionError::ProviderRequired);
        assert_error(&SteerError::EmptyInput);
    }

    #[derive(Default)]
    struct ControlledExecutor {
        started: Notify,
        release: Notify,
        completed: Notify,
    }

    impl SessionExecutor for ControlledExecutor {
        fn send<'a>(&'a self, _session: &'a Session, input: String) -> SessionFuture<'a> {
            Box::pin(async move {
                self.started.notify_one();
                self.release.notified().await;
                self.completed.notify_one();
                Ok(input)
            })
        }
    }

    fn session_state(manager: &SessionManager) -> Arc<SessionState> {
        manager
            .create(SessionOptions::new("provider"), AssembledContext::default())
            .unwrap()
    }

    #[tokio::test]
    async fn sessions_start_with_empty_history() {
        let manager = SessionManager::new();
        let state = session_state(&manager);

        assert!(state.messages.read().await.is_empty());
        assert!(manager.owns(&state));
    }

    #[tokio::test]
    async fn sessions_send_through_their_executor() {
        let state = session_state(&SessionManager::new());
        let session = Session::new(state, Arc::new(EchoExecutor));

        assert_eq!(session.send("hello").await.unwrap(), "hello");
    }

    #[tokio::test]
    async fn session_task_failures_preserve_the_join_error() {
        let state = session_state(&SessionManager::new());
        let session = Session::new(state, Arc::new(PanicExecutor));

        let error = session.send("hello").await.unwrap_err();

        assert!(matches!(&error, RunError::SessionTask { source } if source.is_panic()));
        assert!(
            std::error::Error::source(&error)
                .and_then(|source| source.downcast_ref::<Arc<tokio::task::JoinError>>())
                .filter(|source| source.is_panic())
                .is_some()
        );
    }

    #[test]
    fn sessions_require_a_provider() {
        let error = SessionManager::new()
            .create(SessionOptions::new(""), AssembledContext::default())
            .err()
            .expect("an empty provider should fail");

        assert!(matches!(error, SessionError::ProviderRequired));
    }

    #[tokio::test]
    async fn session_runs_outlive_their_callers() {
        let state = session_state(&SessionManager::new());
        let executor = Arc::new(ControlledExecutor::default());
        let session = Session::new(state, executor.clone());

        let caller = tokio::spawn(async move { session.send("hello").await });
        executor.started.notified().await;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());

        executor.release.notify_one();
        timeout(Duration::from_secs(1), executor.completed.notified())
            .await
            .expect("session run should continue after its caller is cancelled");
    }

    #[tokio::test]
    async fn sessions_can_discard_queued_steering() {
        let state = session_state(&SessionManager::new());
        let session = Session::new(Arc::clone(&state), Arc::new(EchoExecutor));
        let mut events = session.subscribe();

        assert_eq!(
            session.steer("too early"),
            Err(SteerError::NoActiveRunToSteer)
        );
        assert_eq!(session.interrupt(), Err(SteerError::NoActiveRunToInterrupt));
        assert_eq!(session.steer("  \n"), Err(SteerError::EmptyInput));

        let run = state.start_run();
        let discarded = session.steer("another detail").unwrap();
        session.discard_steering(discarded).unwrap();
        assert_eq!(
            session.discard_steering(discarded),
            Err(SteerError::NotQueued { id: discarded })
        );
        let abandoned = session.steer("never applied").unwrap();
        drop(run);

        assert_eq!(
            [
                events.recv().await.unwrap().unwrap().kind,
                events.recv().await.unwrap().unwrap().kind,
                events.recv().await.unwrap().unwrap().kind,
                events.recv().await.unwrap().unwrap().kind,
            ],
            [
                SessionEventKind::SteeringQueued {
                    id: discarded,
                    input: "another detail".to_owned(),
                },
                SessionEventKind::SteeringDiscarded {
                    id: discarded,
                    input: "another detail".to_owned(),
                },
                SessionEventKind::SteeringQueued {
                    id: abandoned,
                    input: "never applied".to_owned(),
                },
                SessionEventKind::SteeringDiscarded {
                    id: abandoned,
                    input: "never applied".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn steering_rejects_a_run_being_interrupted() {
        let state = session_state(&SessionManager::new());
        let session = Session::new(Arc::clone(&state), Arc::new(EchoExecutor));
        let _run = state.start_run();

        session.interrupt().unwrap();

        assert_eq!(
            session.steer("too late"),
            Err(SteerError::RunBeingInterrupted)
        );
    }

    #[tokio::test]
    async fn session_subscribers_observe_events_independently_and_in_order() {
        let state = session_state(&SessionManager::new());
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

        state.emit(started.kind.clone());
        state.emit(completed.kind.clone());

        assert_eq!(first.recv().await.unwrap(), Some(started.clone()));
        assert_eq!(first.recv().await.unwrap(), Some(completed.clone()));
        assert_eq!(second.recv().await.unwrap(), Some(started));
        assert_eq!(second.recv().await.unwrap(), Some(completed));
    }

    #[tokio::test]
    async fn session_subscribers_report_lag() {
        let state = session_state(&SessionManager::new());
        let session = Session::new(Arc::clone(&state), Arc::new(EchoExecutor));
        let mut events = session.subscribe();

        for sequence in 1..=(EVENT_CHANNEL_CAPACITY as u64 + 1) {
            state.emit(SessionEventKind::AssistantMessage {
                text: sequence.to_string(),
            });
        }

        assert_eq!(
            events.recv().await,
            Err(SessionEventError::Lagged { skipped: 1 })
        );
    }
}
