use super::{CapabilityHost, exec};
use crate::{config::Config, consent::ConsentError};
use chap_exec::{
    exec::CommandPrefix,
    host::{CommandTarget, ExecError, ExecOutcome, Executor},
};
use lockgate::{HostCtx, PermissionDenied, PluginSubject, ResolveScopedResource};
use std::{convert::Infallible, sync::Arc, time::Duration};

pub(in crate::agent) fn new(config: &Config) -> Result<Arc<Executor>, ConsentError> {
    let project_root = std::env::current_dir()
        .map_err(|source| ConsentError::CurrentDirectoryUnavailable { source })?;
    Ok(Arc::new(Executor::new(
        config
            .exec_settings()
            .map_err(ConsentError::HostConfiguration)?,
        &project_root,
    )))
}

impl ResolveScopedResource<CommandPrefix, exec::Command> for CapabilityHost {
    type Resource = CommandTarget;
    type Error = Infallible;

    async fn resolve_scoped_resource<'a>(
        &'a self,
        _subject: &'a PluginSubject<'_>,
        command: &'a exec::Command,
    ) -> Result<Self::Resource, Self::Error> {
        Ok(command.into())
    }
}

#[lockgate::guarded]
impl exec::Host for CapabilityHost {
    #[lockgate::requires(permission = chap_exec::exec::RUN, target = command, wire_type = exec::Command)]
    async fn run(
        &mut self,
        cx: HostCtx<'_, ()>,
        command: CommandTarget,
        timeout_ms: Option<u64>,
    ) -> Result<exec::ExecResult, exec::ExecError> {
        self.executor
            .execute(
                cx.subject().plugin_id(),
                &command,
                timeout_ms.map(Duration::from_millis),
            )
            .await
            .map(Into::into)
            .map_err(Into::into)
    }
}

impl From<&exec::Command> for CommandTarget {
    fn from(command: &exec::Command) -> Self {
        Self {
            program: command.program.clone(),
            args: command.args.clone(),
        }
    }
}

impl From<ExecOutcome> for exec::ExecResult {
    fn from(outcome: ExecOutcome) -> Self {
        Self {
            exit_code: outcome.exit_code,
            stdout: outcome.stdout,
            stderr: outcome.stderr,
            truncated: outcome.truncated,
        }
    }
}

impl From<PermissionDenied> for exec::ExecError {
    fn from(error: PermissionDenied) -> Self {
        Self::Denied(format!("{}.{}", error.capability(), error.permission()))
    }
}

impl From<Infallible> for exec::ExecError {
    fn from(error: Infallible) -> Self {
        match error {}
    }
}

impl From<ExecError> for exec::ExecError {
    fn from(error: ExecError) -> Self {
        match error {
            ExecError::Rejected(message) => Self::Rejected(message),
            ExecError::TimedOut => Self::TimedOut,
            ExecError::Failed(message) => Self::Failed(message),
        }
    }
}
