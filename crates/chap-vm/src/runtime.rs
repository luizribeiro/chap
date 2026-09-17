use std::{
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
};

// macOS allows 104 bytes for a Unix socket path; microsandbox adds a 52-byte
// socket suffix, and the terminating NUL consumes one byte.
const MAX_MSB_HOME_BYTES: usize = 104 - 52 - 1;

#[derive(Debug)]
pub enum RuntimeError {
    MsbHomeTooLong {
        path: PathBuf,
        length: usize,
    },
    TargetOccupied {
        path: PathBuf,
    },
    Filesystem {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MsbHomeTooLong { path, length } => write!(
                formatter,
                "MSB_HOME `{}` is {length} bytes, exceeding the {MAX_MSB_HOME_BYTES}-byte limit; set MSB_HOME to a shorter directory",
                path.display()
            ),
            Self::TargetOccupied { path } => write!(
                formatter,
                "cannot create runtime symlink at `{}` because the path exists and is not a symlink",
                path.display()
            ),
            Self::Filesystem {
                action,
                path,
                source,
            } => write!(
                formatter,
                "failed to {action} `{}`: {source}",
                path.display()
            ),
        }
    }
}

impl Error for RuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Filesystem { source, .. } => Some(source),
            Self::MsbHomeTooLong { .. } | Self::TargetOccupied { .. } => None,
        }
    }
}

pub fn msb_home(explicit: Option<PathBuf>, state_root: &Path) -> PathBuf {
    explicit.unwrap_or_else(|| state_root.join("chap/msb"))
}

pub(crate) fn prepare_runtime(msb_home: &Path, runtime: Option<&Path>) -> Result<(), RuntimeError> {
    validate_msb_home(msb_home)?;
    if let Some(runtime) = runtime {
        link_runtime(msb_home, runtime)?;
    }
    Ok(())
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

pub fn link_runtime(msb_home: &Path, runtime: &Path) -> Result<(), RuntimeError> {
    let bin_directory = msb_home.join("bin");
    let lib_directory = msb_home.join("lib");
    create_directory(&bin_directory)?;
    create_directory(&lib_directory)?;

    replace_symlink(&runtime.join("bin/msb"), &bin_directory.join("msb"))?;

    let runtime_lib = runtime.join("lib");
    let entries = fs::read_dir(&runtime_lib).map_err(|source| RuntimeError::Filesystem {
        action: "read runtime directory",
        path: runtime_lib.clone(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| RuntimeError::Filesystem {
            action: "read runtime directory entry in",
            path: runtime_lib.clone(),
            source,
        })?;
        replace_symlink(&entry.path(), &lib_directory.join(entry.file_name()))?;
    }
    Ok(())
}

fn create_directory(path: &Path) -> Result<(), RuntimeError> {
    fs::create_dir_all(path).map_err(|source| RuntimeError::Filesystem {
        action: "create directory",
        path: path.to_path_buf(),
        source,
    })
}

fn replace_symlink(source: &Path, target: &Path) -> Result<(), RuntimeError> {
    match fs::symlink_metadata(target) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            fs::remove_file(target).map_err(|source| RuntimeError::Filesystem {
                action: "remove stale symlink",
                path: target.to_path_buf(),
                source,
            })?;
        }
        Ok(_) => {
            return Err(RuntimeError::TargetOccupied {
                path: target.to_path_buf(),
            });
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(RuntimeError::Filesystem {
                action: "inspect runtime symlink target",
                path: target.to_path_buf(),
                source,
            });
        }
    }

    create_symlink(source, target).map_err(|source| RuntimeError::Filesystem {
        action: "create runtime symlink",
        path: target.to_path_buf(),
        source,
    })
}

#[cfg(unix)]
fn create_symlink(source: &Path, target: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(source, target)
}

#[cfg(windows)]
fn create_symlink(source: &Path, target: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_file(source, target)
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

        prepare_runtime(&path, None).unwrap();
    }

    #[test]
    fn rejects_a_52_byte_msb_home_and_names_it() {
        let path = PathBuf::from("a".repeat(52));

        let error = prepare_runtime(&path, None).unwrap_err();
        let message = error.to_string();

        assert!(message.contains(path.to_str().unwrap()));
        assert!(message.contains("51-byte limit"));
        assert!(message.contains("set MSB_HOME to a shorter directory"));
    }

    #[cfg(unix)]
    mod links {
        use super::*;
        use std::os::unix::fs::symlink;

        fn runtime() -> tempfile::TempDir {
            let runtime = tempfile::tempdir().unwrap();
            fs::create_dir_all(runtime.path().join("bin")).unwrap();
            fs::create_dir_all(runtime.path().join("lib")).unwrap();
            fs::write(runtime.path().join("bin/msb"), "msb").unwrap();
            fs::write(runtime.path().join("lib/libkrunfw.5.dylib"), "library").unwrap();
            runtime
        }

        #[test]
        fn fresh_directory_gets_binary_and_library_links() {
            let runtime = runtime();
            let state = tempfile::tempdir().unwrap();
            let home = state.path().join("msb");

            link_runtime(&home, runtime.path()).unwrap();

            assert_eq!(
                fs::read_link(home.join("bin/msb")).unwrap(),
                runtime.path().join("bin/msb")
            );
            assert_eq!(
                fs::read_link(home.join("lib/libkrunfw.5.dylib")).unwrap(),
                runtime.path().join("lib/libkrunfw.5.dylib")
            );
        }

        #[test]
        fn replaces_a_stale_symlink() {
            let runtime = runtime();
            let state = tempfile::tempdir().unwrap();
            let home = state.path().join("msb");
            fs::create_dir_all(home.join("bin")).unwrap();
            symlink("missing", home.join("bin/msb")).unwrap();

            link_runtime(&home, runtime.path()).unwrap();

            assert_eq!(
                fs::read_link(home.join("bin/msb")).unwrap(),
                runtime.path().join("bin/msb")
            );
        }

        #[test]
        fn regular_file_at_target_is_an_error() {
            let runtime = runtime();
            let state = tempfile::tempdir().unwrap();
            let home = state.path().join("msb");
            fs::create_dir_all(home.join("bin")).unwrap();
            fs::write(home.join("bin/msb"), "occupied").unwrap();

            let error = link_runtime(&home, runtime.path()).unwrap_err();

            assert!(
                error
                    .to_string()
                    .contains(home.join("bin/msb").to_str().unwrap())
            );
        }

        #[test]
        fn links_runtime_aliases_by_name() {
            let runtime = runtime();
            symlink(
                "libkrunfw.5.dylib",
                runtime.path().join("lib/libkrunfw.dylib"),
            )
            .unwrap();
            let state = tempfile::tempdir().unwrap();
            let home = state.path().join("msb");

            link_runtime(&home, runtime.path()).unwrap();

            assert_eq!(
                fs::read_link(home.join("lib/libkrunfw.dylib")).unwrap(),
                runtime.path().join("lib/libkrunfw.dylib")
            );
        }
    }
}
