use std::{
    path::Path,
    process::{Command, ExitCode},
};

fn main() -> ExitCode {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let commands: &[(&Path, &[&str])] = &[
        (
            &workspace,
            &[
                "test",
                "--workspace",
                "--all-targets",
                "--locked",
                "--exclude",
                "chap-openai-compatible",
                "--exclude",
                "chap-exec-plugin",
                "--exclude",
                "chap-state-plugin",
                "--exclude",
                "chap-workspace-plugin",
                "--exclude",
                "chap-kagi",
                "--exclude",
                "chap-persona",
            ],
        ),
        (
            &workspace,
            &[
                "test",
                "-p",
                "chap-core",
                "--features",
                "exec",
                "--all-targets",
                "--locked",
            ],
        ),
        (
            &workspace,
            &[
                "test",
                "-p",
                "chap-core",
                "--features",
                "state",
                "--all-targets",
                "--locked",
            ],
        ),
        (
            &workspace,
            &[
                "test",
                "-p",
                "chap-core",
                "--features",
                "vm",
                "--all-targets",
                "--locked",
            ],
        ),
        (
            &workspace,
            &[
                "test",
                "-p",
                "chap-core",
                "--features",
                "exec,state,vm",
                "--all-targets",
                "--locked",
            ],
        ),
        (
            &workspace,
            &[
                "test",
                "-p",
                "chap-vm",
                "--features",
                "host,microsandbox",
                "--locked",
            ],
        ),
        (
            &workspace.join("plugins/openai-compatible"),
            &["test", "--locked"],
        ),
        (&workspace.join("plugins/exec"), &["test", "--locked"]),
        (&workspace.join("plugins/state"), &["test", "--locked"]),
        (&workspace.join("plugins/workspace"), &["test", "--locked"]),
        (&workspace.join("plugins/kagi"), &["test", "--locked"]),
        (&workspace.join("plugins/persona"), &["test", "--locked"]),
    ];

    for (directory, arguments) in commands {
        let status = Command::new(env!("CARGO"))
            .current_dir(directory)
            .args(*arguments)
            .status();
        match status {
            Ok(status) if status.success() => {}
            Ok(status) => {
                eprintln!("test command failed with {status}");
                return ExitCode::FAILURE;
            }
            Err(error) => {
                eprintln!(
                    "failed to run Cargo from `{}`: {error}",
                    directory.display()
                );
                return ExitCode::FAILURE;
            }
        }
    }

    ExitCode::SUCCESS
}
