use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
};

// macOS allows 104 bytes for a Unix socket path; microsandbox adds a 52-byte
// socket suffix, and the terminating NUL consumes one byte.
const MAX_MSB_HOME_BYTES: usize = 104 - 52 - 1;

#[derive(Debug)]
pub enum RuntimeError {
    MsbHomeTooLong { path: PathBuf, length: usize },
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MsbHomeTooLong { path, length } => write!(
                formatter,
                "MSB_HOME `{}` is {length} bytes, exceeding the {MAX_MSB_HOME_BYTES}-byte limit; set MSB_HOME to a shorter directory",
                path.display()
            ),
        }
    }
}

impl Error for RuntimeError {}

pub fn msb_home(explicit: Option<PathBuf>, state_root: &Path) -> PathBuf {
    explicit.unwrap_or_else(|| state_root.join("chap/msb"))
}

pub(crate) fn prepare_runtime(msb_home: &Path) -> Result<(), RuntimeError> {
    validate_msb_home(msb_home)
}

fn validate_msb_home(path: &Path) -> Result<(), RuntimeError> {
    let length = path.as_os_str().as_encoded_bytes().len();
    if length > MAX_MSB_HOME_BYTES {
        return Err(RuntimeError::MsbHomeTooLong {
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
    fn explicit_msb_home_wins() {
        let path = msb_home(Some(PathBuf::from("explicit")), Path::new("state"));

        assert_eq!(path, Path::new("explicit"));
    }

    #[test]
    fn default_msb_home_uses_chap_state_directory() {
        let path = msb_home(None, Path::new("state"));

        assert_eq!(path, Path::new("state/chap/msb"));
    }

    #[test]
    fn accepts_a_51_byte_msb_home() {
        let path = PathBuf::from("a".repeat(51));

        prepare_runtime(&path).unwrap();
    }

    #[test]
    fn rejects_a_52_byte_msb_home_and_names_it() {
        let path = PathBuf::from("a".repeat(52));

        let error = prepare_runtime(&path).unwrap_err();
        let message = error.to_string();

        assert!(message.contains(path.to_str().unwrap()));
        assert!(message.contains("51-byte limit"));
        assert!(message.contains("set MSB_HOME to a shorter directory"));
    }
}
