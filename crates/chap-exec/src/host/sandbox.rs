use tokio::process::Command;

pub(super) enum Sandbox {
    None,
}

impl Sandbox {
    pub(super) fn wrap(&self, command: Command) -> Command {
        match self {
            Self::None => command,
        }
    }
}
