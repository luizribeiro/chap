use std::format;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::vec::Vec;

use super::ExecError;

pub(super) fn pinned_search_path(configured: Option<Vec<PathBuf>>) -> Vec<PathBuf> {
    configured
        .unwrap_or_else(|| {
            std::env::var_os("PATH")
                .map(|path| std::env::split_paths(&path).collect())
                .unwrap_or_default()
        })
        .into_iter()
        .map(|directory| std::path::absolute(&directory).unwrap_or(directory))
        .collect()
}

pub(super) fn program(program: &str, path: &[PathBuf]) -> Result<PathBuf, ExecError> {
    if program.contains(['/', '\\']) {
        return Err(ExecError::Rejected(format!(
            "program {program:?} contains a path separator; run project-local scripts through an allowed interpreter, for example `sh scripts/foo.sh`"
        )));
    }

    for directory in path {
        let candidate = directory.join(program);
        let Ok(metadata) = fs::metadata(&candidate) else {
            continue;
        };
        if metadata.is_file()
            && metadata.permissions().mode() & 0o111 != 0
            && let Ok(absolute) = std::path::absolute(candidate)
        {
            return Ok(absolute);
        }
    }

    Err(ExecError::Rejected(format!(
        "program {program:?} was not found on the pinned PATH; configure exec.path with a directory containing an executable regular file"
    )))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::vec;

    use tempfile::TempDir;

    use super::{pinned_search_path, program};
    use crate::host::ExecError;

    fn write_executable(directory: &Path, name: &str) {
        let path = directory.join(name);
        fs::write(&path, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn rejects_programs_with_path_separators() {
        for invalid in ["./x", "a/b", "/usr/bin/git", r"a\b"] {
            let error = program(invalid, &[]).unwrap_err();
            assert!(
                matches!(error, ExecError::Rejected(ref message) if message.contains("path separator") && message.contains("sh scripts/foo.sh")),
                "unexpected error for {invalid:?}: {error:?}"
            );
        }
    }

    #[test]
    fn resolves_the_first_executable_on_the_pinned_path() {
        let first = TempDir::new().unwrap();
        let second = TempDir::new().unwrap();
        write_executable(second.path(), "tool");

        assert_eq!(
            program("tool", &[first.path().into(), second.path().into()]).unwrap(),
            std::path::absolute(second.path().join("tool")).unwrap()
        );

        write_executable(first.path(), "tool");
        assert_eq!(
            program("tool", &[first.path().into(), second.path().into()]).unwrap(),
            std::path::absolute(first.path().join("tool")).unwrap()
        );
    }

    #[test]
    fn ignores_the_ambient_path() {
        let pinned = TempDir::new().unwrap();
        assert!(matches!(
            program("sh", &[pinned.path().into()]),
            Err(ExecError::Rejected(_))
        ));
    }

    #[test]
    fn skips_non_executable_files() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("tool");
        fs::write(&path, "not executable").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        assert!(matches!(
            program("tool", &[directory.path().into()]),
            Err(ExecError::Rejected(_))
        ));
    }

    #[test]
    fn not_found_error_names_the_program_and_pinned_path() {
        let error = program("missing-tool", &[]).unwrap_err();
        assert!(matches!(
            error,
            ExecError::Rejected(message)
                if message.contains("missing-tool") && message.contains("pinned PATH")
        ));
    }

    #[test]
    fn configured_path_replaces_the_ambient_snapshot() {
        let configured = vec!["/one".into(), "/two".into()];
        assert_eq!(pinned_search_path(Some(configured.clone())), configured);
    }

    #[test]
    fn pinned_search_path_makes_relative_entries_absolute() {
        assert!(pinned_search_path(Some(vec!["relative".into()]))[0].is_absolute());
    }
}
