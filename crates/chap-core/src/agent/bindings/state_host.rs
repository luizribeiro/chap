use super::{CapabilityHost, state};
use crate::{config::Config, consent::ConsentError};
use chap_state::host::StateStore;
use lockgate::{HostCtx, PermissionDenied};

pub(in crate::agent) fn new(config: &Config) -> Result<StateStore, ConsentError> {
    Ok(StateStore::new(
        config
            .state_settings()
            .map_err(ConsentError::HostConfiguration)?,
    ))
}

#[lockgate::guarded]
impl state::Host for CapabilityHost {
    #[lockgate::requires(permission = chap_state::state::ACCESS)]
    async fn get(
        &mut self,
        cx: HostCtx<'_, ()>,
        key: String,
    ) -> Result<Option<Vec<u8>>, state::StateError> {
        self.store
            .get(cx.subject().plugin_id().as_str(), &key)
            .map_err(Into::into)
    }

    #[lockgate::requires(permission = chap_state::state::ACCESS)]
    async fn set(
        &mut self,
        cx: HostCtx<'_, ()>,
        key: String,
        value: Vec<u8>,
    ) -> Result<(), state::StateError> {
        self.store
            .set(cx.subject().plugin_id().as_str(), &key, value)
            .map_err(Into::into)
    }

    #[lockgate::requires(permission = chap_state::state::ACCESS)]
    async fn delete(&mut self, cx: HostCtx<'_, ()>, key: String) -> Result<(), state::StateError> {
        self.store
            .delete(cx.subject().plugin_id().as_str(), &key)
            .map_err(Into::into)
    }
}

impl From<PermissionDenied> for state::StateError {
    fn from(error: PermissionDenied) -> Self {
        Self::Denied(format!("{}.{}", error.capability(), error.permission()))
    }
}

impl From<chap_state::host::StateError> for state::StateError {
    fn from(error: chap_state::host::StateError) -> Self {
        match error {
            chap_state::host::StateError::QuotaExceeded(limit) => Self::QuotaExceeded(limit),
            chap_state::host::StateError::InvalidKey => Self::InvalidKey,
        }
    }
}
