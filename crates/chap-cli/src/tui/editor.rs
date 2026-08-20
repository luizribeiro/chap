use std::{fs, io::Write, process::Command};
use tempfile::Builder;

pub fn edit(draft: &str) -> Result<String, String> {
    let editor = std::env::var("EDITOR").map_err(|_| "EDITOR is not set".to_owned())?;
    edit_with(&editor, draft)
}

fn edit_with(editor: &str, draft: &str) -> Result<String, String> {
    let mut command =
        shlex::split(editor).ok_or_else(|| "EDITOR has invalid quoting".to_owned())?;
    if command.is_empty() {
        return Err("EDITOR is empty".to_owned());
    }

    let mut file = Builder::new()
        .prefix("sage-")
        .suffix(".md")
        .tempfile()
        .map_err(|error| format!("failed to create editor file: {error}"))?;
    file.write_all(draft.as_bytes())
        .and_then(|_| file.flush())
        .map_err(|error| format!("failed to write editor file: {error}"))?;

    let program = command.remove(0);
    let status = Command::new(&program)
        .args(command)
        .arg(file.path())
        .status()
        .map_err(|error| format!("failed to run {program}: {error}"))?;
    if !status.success() {
        return Err(match status.code() {
            Some(code) => format!("{program} exited with status {code}"),
            None => format!("{program} was terminated by a signal"),
        });
    }

    let edited = fs::read_to_string(file.path())
        .map_err(|error| format!("failed to read editor file: {error}"))?;
    Ok(remove_final_newline(edited))
}

fn remove_final_newline(mut text: String) -> String {
    if text.ends_with('\n') {
        text.pop();
        if text.ends_with('\r') {
            text.pop();
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_one_editor_added_newline() {
        assert_eq!(remove_final_newline("hello\n\n".into()), "hello\n");
        assert_eq!(remove_final_newline("hello\r\n".into()), "hello");
        assert_eq!(remove_final_newline("hello".into()), "hello");
    }

    #[cfg(unix)]
    #[test]
    fn edits_the_draft_with_editor_arguments() {
        let edited = edit_with("sh -c 'printf edited > \"$1\"' sh", "original").unwrap();
        assert_eq!(edited, "edited");
    }

    #[cfg(unix)]
    #[test]
    fn reports_an_unsuccessful_editor() {
        let error = edit_with("sh -c 'exit 7'", "original").unwrap_err();
        assert!(error.ends_with("exited with status 7"));
    }
}
