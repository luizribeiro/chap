//! Process execution types and functions from the `chap:agent/exec` interface.

mod bindings {
    lockgate_plugin::__wit_bindgen::generate!({
        path: "../chap-wit/wit",
        world: "sdk-exec",
        runtime_path: "lockgate_plugin::__wit_bindgen::rt",
    });
}

pub use bindings::chap::agent::exec::*;

use crate::roles::tools::ToolError;

impl From<ExecError> for ToolError {
    fn from(error: ExecError) -> Self {
        match error {
            ExecError::Denied(message) => {
                Self::Denied(format!("command denied by the capability guard: {message}"))
            }
            ExecError::Rejected(message) => {
                Self::InvalidInput(format!("command rejected: {message}"))
            }
            ExecError::TimedOut => {
                Self::Failed("command timed out and was killed at the deadline".to_owned())
            }
            ExecError::Failed(message) => {
                Self::Failed(format!("command execution failed: {message}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_each_exec_error_variant() {
        assert_eq!(
            ToolError::from(ExecError::Denied("exec.run".to_owned())),
            ToolError::Denied("command denied by the capability guard: exec.run".to_owned())
        );
        assert_eq!(
            ToolError::from(ExecError::Rejected(
                "program paths are not allowed".to_owned()
            )),
            ToolError::InvalidInput("command rejected: program paths are not allowed".to_owned())
        );
        assert_eq!(
            ToolError::from(ExecError::TimedOut),
            ToolError::Failed("command timed out and was killed at the deadline".to_owned())
        );
        assert_eq!(
            ToolError::from(ExecError::Failed("could not spawn".to_owned())),
            ToolError::Failed("command execution failed: could not spawn".to_owned())
        );
    }
}
