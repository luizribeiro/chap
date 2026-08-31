use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::format;
use std::path::PathBuf;

use super::ExecError;

const PINNED_KEYS: [&str; 4] = ["HOME", "TERM", "LANG", "TMPDIR"];

pub(super) fn constructed(path: &[PathBuf]) -> Result<BTreeMap<OsString, OsString>, ExecError> {
    constructed_from(path, |name| std::env::var_os(name))
}

fn constructed_from(
    path: &[PathBuf],
    lookup: impl Fn(&OsStr) -> Option<OsString>,
) -> Result<BTreeMap<OsString, OsString>, ExecError> {
    // The chap process holds provider API keys, so a constructed environment keeps them out of spawned build scripts.
    let mut values = BTreeMap::new();
    for name in PINNED_KEYS {
        if let Some(value) = lookup(OsStr::new(name)) {
            values.insert(OsString::from(name), value);
        }
    }
    let joined = std::env::join_paths(path).map_err(|error| {
        ExecError::Failed(format!(
            "could not construct the pinned PATH for a command: {error}"
        ))
    })?;
    values.insert(OsString::from("PATH"), joined);
    Ok(values)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::{OsStr, OsString};
    use std::path::PathBuf;
    use std::vec;

    use super::constructed_from;

    #[test]
    fn process_environment_is_not_inherited() {
        let canary = "CHAP_EXEC_TEST_PROVIDER_CANARY";
        unsafe {
            std::env::set_var(canary, "secret");
        }

        let result = super::constructed(&[PathBuf::from("/pinned")]);

        unsafe {
            std::env::remove_var(canary);
        }
        let values = result.unwrap();
        assert!(!values.contains_key(OsStr::new(canary)));
    }

    #[test]
    fn constructs_only_pinned_values() {
        let source = BTreeMap::from([
            (OsString::from("HOME"), OsString::from("/home/chap")),
            (OsString::from("TERM"), OsString::from("xterm-256color")),
            (OsString::from("LANG"), OsString::from("en_US.UTF-8")),
            (OsString::from("TMPDIR"), OsString::from("/tmp/chap")),
            (OsString::from("PROVIDER_API_KEY"), OsString::from("canary")),
            (OsString::from("PATH"), OsString::from("/ambient")),
        ]);
        let path = vec![PathBuf::from("/pinned/one"), PathBuf::from("/pinned/two")];
        let values = constructed_from(&path, |name: &OsStr| source.get(name).cloned()).unwrap();

        assert_eq!(values.get(OsStr::new("HOME")).unwrap(), "/home/chap");
        assert_eq!(values.get(OsStr::new("TERM")).unwrap(), "xterm-256color");
        assert_eq!(values.get(OsStr::new("LANG")).unwrap(), "en_US.UTF-8");
        assert_eq!(values.get(OsStr::new("TMPDIR")).unwrap(), "/tmp/chap");
        assert_eq!(
            values.get(OsStr::new("PATH")).unwrap(),
            &std::env::join_paths(&path).unwrap()
        );
        assert!(!values.contains_key(OsStr::new("PROVIDER_API_KEY")));
        assert_eq!(values.len(), 5);
    }

    #[test]
    fn missing_pinned_values_are_absent() {
        let values = constructed_from(&[], |_| None).unwrap();
        assert_eq!(values.len(), 1);
        assert_eq!(values.get(OsStr::new("PATH")).unwrap(), "");
    }

    #[test]
    fn pinned_path_is_never_taken_from_the_environment() {
        let path = vec![PathBuf::from("/pinned")];
        let values = constructed_from(&path, |_| Some(OsString::from("/ambient"))).unwrap();
        assert_eq!(
            values.get(OsStr::new("PATH")).unwrap(),
            &std::env::join_paths(&path).unwrap()
        );
    }
}
