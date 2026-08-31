use std::{sync::Arc, time::Duration};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum FinishReason {
    Stop,
    ToolCalls,
    Length,
    Other(String),
}

#[derive(Clone, Debug, Error)]
#[non_exhaustive]
pub enum ProviderError {
    #[error("{message}")]
    RateLimited {
        retry_after: Option<Duration>,
        message: String,
    },
    #[error("{0}")]
    ContextTooLong(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    Refused(String),
    #[error("provider plugin `{provider}` is not configured")]
    NotConfigured { provider: String },
    #[error("provider plugin `{provider}` failed: {source}")]
    RoleUnavailable {
        provider: String,
        #[source]
        source: lockgate::RoleError,
    },
    /// Chap's wall-clock deadline expired before the provider plugin completed.
    #[error("provider plugin `{provider}` timed out after {deadline:?}")]
    TimedOut {
        provider: String,
        deadline: Duration,
        #[source]
        source: Arc<lockgate::CallError>,
    },
    #[error("provider plugin `{provider}` failed: {source}")]
    CallFailed {
        provider: String,
        #[source]
        source: Arc<lockgate::CallError>,
    },
    #[error("{0}")]
    Other(String),
}

impl PartialEq for ProviderError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::RateLimited {
                    retry_after: left_retry,
                    message: left_message,
                },
                Self::RateLimited {
                    retry_after: right_retry,
                    message: right_message,
                },
            ) => left_retry == right_retry && left_message == right_message,
            (Self::ContextTooLong(left), Self::ContextTooLong(right))
            | (Self::Unauthorized(left), Self::Unauthorized(right))
            | (Self::Unavailable(left), Self::Unavailable(right))
            | (Self::Refused(left), Self::Refused(right))
            | (Self::Other(left), Self::Other(right)) => left == right,
            (Self::NotConfigured { provider: left }, Self::NotConfigured { provider: right }) => {
                left == right
            }
            (
                Self::RoleUnavailable {
                    provider: left_provider,
                    source: left_source,
                },
                Self::RoleUnavailable {
                    provider: right_provider,
                    source: right_source,
                },
            ) => left_provider == right_provider && left_source == right_source,
            (
                Self::TimedOut {
                    provider: left_provider,
                    deadline: left_deadline,
                    source: left_source,
                },
                Self::TimedOut {
                    provider: right_provider,
                    deadline: right_deadline,
                    source: right_source,
                },
            ) => {
                left_provider == right_provider
                    && left_deadline == right_deadline
                    && Arc::ptr_eq(left_source, right_source)
            }
            (
                Self::CallFailed {
                    provider: left_provider,
                    source: left_source,
                },
                Self::CallFailed {
                    provider: right_provider,
                    source: right_source,
                },
            ) => left_provider == right_provider && Arc::ptr_eq(left_source, right_source),
            _ => false,
        }
    }
}

impl Eq for ProviderError {}
