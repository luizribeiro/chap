use std::{fmt, time::Duration};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderError {
    RateLimited {
        retry_after: Option<Duration>,
        message: String,
    },
    ContextTooLong(String),
    Unauthorized(String),
    Unavailable(String),
    Refused(String),
    /// The plugin itself failed: it trapped, exhausted its budget, or could not be
    /// instantiated or dispatched. Never retry this without operator involvement.
    /// This has no WIT counterpart because a guest cannot report its own trap.
    Plugin(String),
    Other(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RateLimited { message, .. }
            | Self::ContextTooLong(message)
            | Self::Unauthorized(message)
            | Self::Unavailable(message)
            | Self::Refused(message)
            | Self::Plugin(message)
            | Self::Other(message) => formatter.write_str(message),
        }
    }
}
