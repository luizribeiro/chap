use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
};

// macOS allows 104 bytes for a Unix socket path; microsandbox adds a 52-byte
// socket suffix, and the terminating NUL consumes one byte.
const MAX_STATE_DIR_BYTES: usize = 104 - 52 - 1;

#[derive(Debug)]
pub enum RuntimeError {
    StateDirTooLong { path: PathBuf, length: usize },
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StateDirTooLong { path, length } => write!(
                formatter,
                "chap's sandbox state directory `{}` is {length} bytes, over the {MAX_STATE_DIR_BYTES}-byte limit for VM socket paths; set XDG_STATE_HOME to a shorter directory",
                path.display()
            ),
        }
    }
}

impl Error for RuntimeError {}

pub fn sandbox_state_dir(state_root: &Path) -> PathBuf {
    state_root.join("chap/msb")
}

pub(crate) fn validate_state_dir(path: &Path) -> Result<(), RuntimeError> {
    let length = path.as_os_str().as_encoded_bytes().len();
    if length > MAX_STATE_DIR_BYTES {
        return Err(RuntimeError::StateDirTooLong {
            path: path.to_path_buf(),
            length,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::ToString;

    #[test]
    fn the_sandbox_state_directory_defaults_to_chap_state() {
        let path = sandbox_state_dir(Path::new("state"));

        assert_eq!(path, Path::new("state/chap/msb"));
    }

    #[test]
    fn accepts_a_51_byte_state_directory() {
        let path = PathBuf::from("a".repeat(51));

        validate_state_dir(&path).unwrap();
    }

    #[test]
    fn rejects_a_52_byte_state_directory_and_names_xdg_state_home() {
        let path = PathBuf::from("a".repeat(52));

        let error = validate_state_dir(&path).unwrap_err();
        let message = error.to_string();

        assert!(message.contains(path.to_str().unwrap()));
        assert!(message.contains("51-byte limit"));
        assert!(message.contains("set XDG_STATE_HOME to a shorter directory"));
    }
}
