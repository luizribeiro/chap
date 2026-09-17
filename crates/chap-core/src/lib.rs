mod agent;
mod config;
mod consent;
mod provider;
mod session;
mod tool;

pub use agent::{
    Agent, AgentBuilder, PluginCheck, PluginRefusal, PluginRefusalReason, StartError,
    WorkspaceInfo, WorkspaceSecret,
};
pub use config::{LoadError, state_root};
pub use consent::{ConsentError, ConsentStore, PluginConsentReview};
pub use lockgate::{
    CallBudget, ConsentManifest, ConsentRecord, DriftChange, DriftKind, DriftReport, ExportDrift,
    ExportDriftKind, GrantReview, PluginId, RequiredEnvironmentVariable,
};
pub use provider::{FinishReason, ProviderError};
pub use session::{
    ContextError, ContextFailure, RunError, RunUsage, Session, SessionError, SessionEvent,
    SessionEventError, SessionEventKind, SessionEvents, SessionId, SessionOptions, SteerError,
    SteeringId, Usage,
};
pub use tool::{ExecutionMode, Tool, ToolDefinition, ToolError, ToolRegistrationError};

/// Installs `ring` as the process-wide rustls crypto provider.
///
/// rustls picks a provider on its own only when exactly one of its `ring` and
/// `aws-lc-rs` features is compiled in. A binary with the microsandbox backend
/// has both: wasmtime-wasi-http and the microsandbox crates enable `ring`, while
/// reqwest, pulled in by microsandbox and oci-client, enables `aws-lc-rs`. With
/// both present, the first `ClientConfig::builder()` anywhere in the process
/// panics unless a default was installed beforehand. Cargo features are
/// additive, so neither side can be switched off from this workspace; the
/// binary has to choose, and it has to do so before any TLS client is built.
///
/// Calling this more than once is harmless: a second install fails and the
/// error is ignored.
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
