use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::format;
use std::path::PathBuf;
use std::string::String;

use super::ExecError;

const PINNED_KEYS: [&str; 4] = ["HOME", "TERM", "LANG", "TMPDIR"];

pub(super) fn constructed(
    path: &[PathBuf],
    passthrough: &[String],
) -> Result<BTreeMap<OsString, OsString>, ExecError> {
    constructed_from(path, passthrough, |name| std::env::var_os(name))
}

fn constructed_from(
    path: &[PathBuf],
    passthrough: &[String],
    lookup: impl Fn(&OsStr) -> Option<OsString>,
) -> Result<BTreeMap<OsString, OsString>, ExecError> {
    // The chap process holds provider API keys, so a constructed environment keeps them out of spawned build scripts.
    let mut values = BTreeMap::new();
    for name in passthrough {
        if let Some(value) = lookup(OsStr::new(name)) {
            values.insert(OsString::from(name), value);
        }
    }
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
    use std::string::String;
    use std::vec;

    use super::constructed_from;

    #[test]
    fn process_canary_is_absent_and_passthrough_is_copied() {
        let canary = "CHAP_EXEC_TEST_PROVIDER_CANARY";
        let allowed = "CHAP_EXEC_TEST_ALLOWED";
        unsafe {
            std::env::set_var(canary, "secret");
            std::env::set_var(allowed, "present");
        }

        let result = super::constructed(&[PathBuf::from("/pinned")], &[String::from(allowed)]);

        unsafe {
            std::env::remove_var(canary);
            std::env::remove_var(allowed);
        }
        let values = result.unwrap();
        assert!(!values.contains_key(OsStr::new(canary)));
        assert_eq!(values.get(OsStr::new(allowed)).unwrap(), "present");
    }

    #[test]
    fn constructs_only_pinned_and_passthrough_values() {
        let source = BTreeMap::from([
            (OsString::from("HOME"), OsString::from("/home/chap")),
            (OsString::from("TERM"), OsString::from("xterm-256color")),
            (OsString::from("LANG"), OsString::from("en_US.UTF-8")),
            (OsString::from("TMPDIR"), OsString::from("/tmp/chap")),
            (OsString::from("ALLOWED"), OsString::from("present")),
            (OsString::from("PROVIDER_API_KEY"), OsString::from("canary")),
            (OsString::from("PATH"), OsString::from("/ambient")),
        ]);
        let path = vec![PathBuf::from("/pinned/one"), PathBuf::from("/pinned/two")];
        let values = constructed_from(&path, &[String::from("ALLOWED")], |name: &OsStr| {
            source.get(name).cloned()
        })
        .unwrap();

        assert_eq!(values.get(OsStr::new("HOME")).unwrap(), "/home/chap");
        assert_eq!(values.get(OsStr::new("TERM")).unwrap(), "xterm-256color");
        assert_eq!(values.get(OsStr::new("LANG")).unwrap(), "en_US.UTF-8");
        assert_eq!(values.get(OsStr::new("TMPDIR")).unwrap(), "/tmp/chap");
        assert_eq!(values.get(OsStr::new("ALLOWED")).unwrap(), "present");
        assert_eq!(
            values.get(OsStr::new("PATH")).unwrap(),
            &std::env::join_paths(&path).unwrap()
        );
        assert!(!values.contains_key(OsStr::new("PROVIDER_API_KEY")));
        assert_eq!(values.len(), 6);
    }

    #[test]
    fn missing_pinned_and_passthrough_values_are_absent() {
        let values = constructed_from(&[], &[String::from("MISSING")], |_| None).unwrap();
        assert_eq!(values.len(), 1);
        assert_eq!(values.get(OsStr::new("PATH")).unwrap(), "");
    }

    #[test]
    fn passthrough_cannot_replace_the_pinned_path() {
        let path = vec![PathBuf::from("/pinned")];
        let values = constructed_from(&path, &[String::from("PATH")], |_| {
            Some(OsString::from("/ambient"))
        })
        .unwrap();
        assert_eq!(
            values.get(OsStr::new("PATH")).unwrap(),
            &std::env::join_paths(&path).unwrap()
        );
    }
}
