//! Key-value state types and functions from the `chap:agent/state` interface.

mod bindings {
    lockgate_plugin::__wit_bindgen::generate!({
        path: "../chap-wit/wit",
        world: "sdk-state",
        runtime_path: "lockgate_plugin::__wit_bindgen::rt",
    });
}

pub use bindings::chap::agent::state::*;

use crate::roles::tools::ToolError;

impl From<StateError> for ToolError {
    fn from(error: StateError) -> Self {
        match error {
            StateError::Denied(message) => Self::Denied(format!(
                "state access denied by the capability guard: {message}"
            )),
            StateError::QuotaExceeded(limit) => {
                Self::Failed(format!("state store quota of {limit} bytes exceeded"))
            }
            StateError::InvalidKey => Self::InvalidInput("state key must not be empty".to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_each_state_error_variant() {
        assert_eq!(
            ToolError::from(StateError::Denied("state.access".to_owned())),
            ToolError::Denied(
                "state access denied by the capability guard: state.access".to_owned()
            )
        );
        assert_eq!(
            ToolError::from(StateError::QuotaExceeded(1_048_576)),
            ToolError::Failed("state store quota of 1048576 bytes exceeded".to_owned())
        );
        assert_eq!(
            ToolError::from(StateError::InvalidKey),
            ToolError::InvalidInput("state key must not be empty".to_owned())
        );
    }
}
