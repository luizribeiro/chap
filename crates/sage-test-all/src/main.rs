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
                "sage-openai-compatible",
                "--exclude",
                "sage-kagi",
            ],
        ),
        (
            &workspace.join("plugins/openai-compatible"),
            &["test", "--locked"],
        ),
        (&workspace.join("plugins/kagi"), &["test", "--locked"]),
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
