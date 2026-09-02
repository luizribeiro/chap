mod agent;
mod config;
mod consent;
mod provider;
mod session;
mod tool;

pub use agent::{Agent, AgentBuilder, PluginCheck, PluginRefusal, PluginRefusalReason, StartError};
pub use config::LoadError;
pub use consent::{ConsentError, ConsentStore, PluginConsentReview};
pub use lockgate::{
    CallBudget, ConsentManifest, ConsentRecord, DriftChange, DriftKind, DriftReport, ExportDrift,
    ExportDriftKind, GrantReview, RequiredEnvironmentVariable,
};
pub use provider::{FinishReason, ProviderError};
pub use session::{
    ContextError, ContextFailure, RunError, RunUsage, Session, SessionError, SessionEvent,
    SessionEventError, SessionEventKind, SessionEvents, SessionId, SessionOptions, SteerError,
    SteeringId, Usage,
};
pub use tool::{ExecutionMode, Tool, ToolDefinition, ToolError, ToolRegistrationError};

/// Installs the crypto provider used by Chap's TLS clients.
///
/// Calling this more than once is harmless.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[cfg(test)]
mod tests {
    #[test]
    fn installs_the_crypto_provider_idempotently() {
        super::install_crypto_provider();
        assert!(rustls::crypto::CryptoProvider::get_default().is_some());

        super::install_crypto_provider();
    }
}
