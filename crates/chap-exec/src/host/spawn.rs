use std::format;
use std::io;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::string::String;
use std::time::Duration;
use std::vec::Vec;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use super::{ExecError, ExecOutcome};

const OUTPUT_LIMIT: usize = 64 * 1024;
const TRUNCATION_MARKER: &[u8] = b"\n[chap-exec output truncated at 64 KiB]\n";

pub(super) async fn run(
    mut command: Command,
    project_root: &Path,
    timeout: Duration,
) -> Result<ExecOutcome, ExecError> {
    command
        .current_dir(project_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);

    let program = command
        .as_std()
        .get_program()
        .to_string_lossy()
        .into_owned();
    let mut child = command.spawn().map_err(|error| {
        ExecError::Failed(format!("could not spawn program {program:?}: {error}"))
    })?;
    let pid = child.id().ok_or_else(|| {
        ExecError::Failed(format!(
            "could not supervise program {program:?}: its process ID was unavailable"
        ))
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        ExecError::Failed(format!("could not capture stdout from program {program:?}"))
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        ExecError::Failed(format!("could not capture stderr from program {program:?}"))
    })?;
    let stdout_task = tokio::spawn(read_capped(stdout));
    let stderr_task = tokio::spawn(read_capped(stderr));

    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(error)) => {
            stdout_task.abort();
            stderr_task.abort();
            return Err(ExecError::Failed(format!(
                "could not wait for program {program:?}: {error}"
            )));
        }
        Err(_) => {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
            let _ = child.wait().await;
            let _ = stdout_task.await;
            let _ = stderr_task.await;
            return Err(ExecError::TimedOut);
        }
    };

    let stdout = captured(stdout_task, "stdout", &program).await?;
    let stderr = captured(stderr_task, "stderr", &program).await?;
    Ok(ExecOutcome {
        exit_code: exit_code(status),
        stdout: String::from_utf8_lossy(&stdout.bytes).into_owned(),
        stderr: String::from_utf8_lossy(&stderr.bytes).into_owned(),
        truncated: stdout.truncated || stderr.truncated,
    })
}

struct Captured {
    bytes: Vec<u8>,
    truncated: bool,
}

async fn read_capped(mut reader: impl AsyncRead + Unpin) -> Result<Captured, io::Error> {
    let mut bytes = Vec::with_capacity(OUTPUT_LIMIT);
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;

    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let remaining = OUTPUT_LIMIT.saturating_sub(bytes.len());
        bytes.extend_from_slice(&buffer[..read.min(remaining)]);
        if read > remaining || remaining == 0 {
            truncated = true;
        }
    }

    if truncated {
        bytes.truncate(OUTPUT_LIMIT - TRUNCATION_MARKER.len());
        bytes.extend_from_slice(TRUNCATION_MARKER);
    }
    Ok(Captured { bytes, truncated })
}

async fn captured(
    task: tokio::task::JoinHandle<Result<Captured, io::Error>>,
    stream: &str,
    program: &str,
) -> Result<Captured, ExecError> {
    task.await
        .map_err(|error| {
            ExecError::Failed(format!(
                "could not capture {stream} from program {program:?}: {error}"
            ))
        })?
        .map_err(|error| {
            ExecError::Failed(format!(
                "could not read {stream} from program {program:?}: {error}"
            ))
        })
}

fn exit_code(status: ExitStatus) -> i32 {
    status.code().unwrap_or_else(|| {
        // Shell-style signal codes make command outcomes familiar to callers and logs.
        128 + status.signal().unwrap_or(0)
    })
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::path::{Path, PathBuf};
    use std::string::String;
    use std::time::{Duration, Instant};
    use std::vec::Vec;

    use tempfile::TempDir;
    use tokio::process::Command;

    use super::{OUTPUT_LIMIT, TRUNCATION_MARKER, run};
    use crate::host::{ExecError, resolve};

    fn resolved(name: &str) -> PathBuf {
        let path: Vec<PathBuf> = env::var_os("PATH")
            .map(|value| env::split_paths(&value).collect())
            .unwrap_or_default();
        resolve::program(name, &path).unwrap()
    }

    fn command(name: &str, args: &[&str]) -> Command {
        let mut command = Command::new(resolved(name));
        command.args(args);
        command.env_clear();
        if let Some(path) = env::var_os("PATH") {
            command.env("PATH", path);
        }
        command
    }

    async fn execute(
        project: &Path,
        name: &str,
        args: &[&str],
        timeout: Duration,
    ) -> Result<crate::host::ExecOutcome, ExecError> {
        run(command(name, args), project, timeout).await
    }

    #[tokio::test]
    async fn captures_echo_output_and_success() {
        let project = TempDir::new().unwrap();
        let outcome = execute(project.path(), "echo", &["hello"], Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(outcome.exit_code, 0, "stderr: {:?}", outcome.stderr);
        assert_eq!(outcome.stdout, "hello\n");
        assert_eq!(outcome.stderr, "");
        assert!(!outcome.truncated);
    }

    #[tokio::test]
    async fn reports_nonzero_exit_codes() {
        let project = TempDir::new().unwrap();
        let outcome = execute(
            project.path(),
            "sh",
            &["-c", "exit 7"],
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(outcome.exit_code, 7);
    }

    #[tokio::test]
    async fn captures_stdout_and_stderr_concurrently() {
        let project = TempDir::new().unwrap();
        let outcome = execute(
            project.path(),
            "sh",
            &["-c", "printf stdout; printf stderr >&2"],
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(outcome.stdout, "stdout");
        assert_eq!(outcome.stderr, "stderr");
    }

    #[tokio::test]
    async fn caps_and_marks_truncated_output() {
        let project = TempDir::new().unwrap();
        let outcome = execute(
            project.path(),
            "sh",
            &[
                "-c",
                "i=0; while [ \"$i\" -lt 70000 ]; do printf x; i=$((i + 1)); done",
            ],
            Duration::from_secs(10),
        )
        .await
        .unwrap();
        assert!(outcome.truncated);
        assert_eq!(outcome.stdout.len(), OUTPUT_LIMIT);
        assert!(
            outcome
                .stdout
                .ends_with(String::from_utf8_lossy(TRUNCATION_MARKER).as_ref())
        );
    }

    #[tokio::test]
    async fn timeout_kills_the_process_group_quickly() {
        let project = TempDir::new().unwrap();
        let started = Instant::now();
        let error = execute(
            project.path(),
            "sh",
            &["-c", "sleep 30"],
            Duration::from_millis(200),
        )
        .await
        .unwrap_err();
        assert_eq!(error, ExecError::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn null_stdin_makes_cat_exit_immediately() {
        let project = TempDir::new().unwrap();
        let started = Instant::now();
        let outcome = execute(project.path(), "cat", &[], Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(outcome.exit_code, 0, "stderr: {:?}", outcome.stderr);
        assert_eq!(outcome.stdout, "");
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn maps_signal_deaths_to_shell_exit_codes() {
        let project = TempDir::new().unwrap();
        let outcome = execute(
            project.path(),
            "sh",
            &["-c", "kill -TERM $$"],
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(outcome.exit_code, 143);
    }
}
