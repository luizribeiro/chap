#[cfg(not(feature = "exec"))]
lockgate::host_bindings!({
    path: "../chap-wit/wit",
    world: "host",
});

#[cfg(feature = "exec")]
lockgate::host_bindings!({
    path: "../chap-wit/wit",
    world: "host-exec",
    imports: ExecImports,
    data: (),
});

#[cfg(feature = "exec")]
pub(super) use exec_host::ExecImports;

#[cfg(feature = "exec")]
mod exec_host {
    use super::exec;
    use chap_exec::{
        exec::CommandPrefix,
        host::{CommandTarget, ExecConfig, ExecError, ExecOutcome, Executor},
    };
    use lockgate::{HostCtx, PermissionDenied, PluginSubject, ResolveScopedResource};
    use std::{convert::Infallible, path::Path, sync::Arc, time::Duration};

    #[derive(Clone)]
    pub(crate) struct ExecImports {
        executor: Arc<Executor>,
    }

    impl ExecImports {
        pub(crate) fn new(config: ExecConfig, project_root: &Path) -> Self {
            Self {
                executor: Arc::new(Executor::new(config, project_root)),
            }
        }
    }

    impl ResolveScopedResource<CommandPrefix, exec::Command> for ExecImports {
        type Resource = CommandTarget;
        type Error = Infallible;

        async fn resolve_scoped_resource<'a>(
            &'a self,
            _subject: &'a PluginSubject<'_>,
            command: &'a exec::Command,
        ) -> Result<Self::Resource, Self::Error> {
            Ok(command_target(command))
        }
    }

    #[lockgate::guarded]
    impl exec::Host for ExecImports {
        #[lockgate::requires(permission = chap_exec::exec::RUN, target = command)]
        async fn run(
            &mut self,
            _cx: HostCtx<'_, ()>,
            command: exec::Command,
            timeout_ms: Option<u64>,
        ) -> Result<exec::ExecResult, exec::ExecError> {
            self.executor
                .execute(
                    &command_target(&command),
                    timeout_ms.map(Duration::from_millis),
                )
                .await
                .map(Into::into)
                .map_err(Into::into)
        }
    }

    // TODO(luizribeiro/lockgate#4): the guard's resolve_scoped_resource and
    // the run body each derive their own CommandTarget through this function,
    // so the checked value and the executed value stay equal only while both
    // remain this one derivation. Execute the guard-approved resource directly
    // once the macro can hand it to the body.
    fn command_target(command: &exec::Command) -> CommandTarget {
        CommandTarget {
            program: command.program.clone(),
            args: command.args.clone(),
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
}
