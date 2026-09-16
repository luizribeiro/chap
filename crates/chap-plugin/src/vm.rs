//! MicroVM types and a VM handle from the `chap:agent/vm` interface.

mod bindings {
    lockgate_plugin::__wit_bindgen::generate!({
        path: "../chap-wit/wit",
        world: "sdk-vm",
        runtime_path: "lockgate_plugin::__wit_bindgen::rt",
    });
}

// The plugin's `with:` mapping targets this module, so raw host calls stay public.
pub use bindings::chap::agent::vm::*;

pub type Workspace = WorkspaceInfo;

use crate::roles::tools::ToolError;

pub struct Vm {
    name: String,
}

impl Vm {
    pub async fn workspace() -> Result<(Self, Workspace), VmError> {
        bindings::chap::agent::vm::workspace()
            .await
            .map(Self::from_workspace)
    }

    fn from_workspace(workspace: Workspace) -> (Self, Workspace) {
        let vm = Self {
            name: workspace.vm.clone(),
        };
        (vm, workspace)
    }

    pub async fn create(name: &str, config: VmConfig) -> Result<Self, VmError> {
        bindings::chap::agent::vm::create(name.to_owned(), config)
            .await
            .map(|name| Self { name })
    }

    pub async fn get(name: &str) -> Result<Option<Self>, VmError> {
        bindings::chap::agent::vm::get(name.to_owned())
            .await
            .map(|name| name.map(|name| Self { name }))
    }

    pub async fn get_or_create(name: &str, config: VmConfig) -> Result<Self, VmError> {
        bindings::chap::agent::vm::get_or_create(name.to_owned(), config)
            .await
            .map(|name| Self { name })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub async fn exec(
        &self,
        command: &[&str],
        cwd: Option<&str>,
        stdin: Option<Vec<u8>>,
        timeout_ms: Option<u64>,
    ) -> Result<ExecResult, VmError> {
        bindings::chap::agent::vm::exec(
            self.name.clone(),
            command.iter().map(|arg| (*arg).to_owned()).collect(),
            cwd.map(str::to_owned),
            stdin,
            timeout_ms,
        )
        .await
    }

    pub async fn read_file(&self, path: &str) -> Result<Vec<u8>, VmError> {
        bindings::chap::agent::vm::read_file(self.name.clone(), path.to_owned()).await
    }

    pub async fn write_file(&self, path: &str, contents: Vec<u8>) -> Result<(), VmError> {
        bindings::chap::agent::vm::write_file(self.name.clone(), path.to_owned(), contents).await
    }

    pub async fn destroy(self) -> Result<(), VmError> {
        bindings::chap::agent::vm::destroy(self.name).await
    }
}

impl From<VmError> for ToolError {
    fn from(error: VmError) -> Self {
        match error {
            VmError::Denied(message) => Self::Denied(format!(
                "vm access denied by the capability guard: {message}"
            )),
            VmError::AlreadyExists => {
                Self::InvalidInput("a vm with that name already exists".to_owned())
            }
            VmError::NoSuchVm => Self::InvalidInput("no such vm".to_owned()),
            VmError::ConfigMismatch => {
                Self::InvalidInput("the vm exists with a different configuration".to_owned())
            }
            VmError::TimedOut => Self::Failed("the vm command timed out".to_owned()),
            VmError::Failed(message) => Self::Failed(format!("vm operation failed: {message}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_each_vm_error_variant() {
        assert_eq!(
            ToolError::from(VmError::Denied("vm.manage".to_owned())),
            ToolError::Denied("vm access denied by the capability guard: vm.manage".to_owned())
        );
        assert_eq!(
            ToolError::from(VmError::AlreadyExists),
            ToolError::InvalidInput("a vm with that name already exists".to_owned())
        );
        assert_eq!(
            ToolError::from(VmError::NoSuchVm),
            ToolError::InvalidInput("no such vm".to_owned())
        );
        assert_eq!(
            ToolError::from(VmError::ConfigMismatch),
            ToolError::InvalidInput("the vm exists with a different configuration".to_owned())
        );
        assert_eq!(
            ToolError::from(VmError::TimedOut),
            ToolError::Failed("the vm command timed out".to_owned())
        );
        assert_eq!(
            ToolError::from(VmError::Failed("agent unavailable".to_owned())),
            ToolError::Failed("vm operation failed: agent unavailable".to_owned())
        );
    }

    #[test]
    fn workspace_handle_comes_from_the_host_record() {
        let workspace = Workspace {
            vm: "@workspace".into(),
            image: "docker.io/library/alpine:3.20".into(),
            mounts: vec![WorkspaceMount {
                guest: "/mnt/workspace".into(),
                readonly: false,
            }],
            egress: vec!["0.0.0.0/0:443".into()],
        };

        let (vm, workspace) = Vm::from_workspace(workspace);

        assert_eq!(vm.name(), "@workspace");
        assert_eq!(workspace.vm, vm.name());
    }
}
