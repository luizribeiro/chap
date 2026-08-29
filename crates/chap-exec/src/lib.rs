#![no_std]

//! Authority contract shared by exec hosts and plugins.

extern crate alloc;

#[cfg(feature = "host")]
extern crate std;

#[cfg(feature = "host")]
pub mod host;

#[lockgate_policy::capability("exec")]
pub mod exec {
    use alloc::{format, string::String, vec::Vec};
    use core::str::FromStr;
    use lockgate_policy::{Scope, ScopeError, ScopeRepr, ScopedPermission};

    /// An ordered argv prefix authorized for process spawning.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct CommandPrefix {
        tokens: Vec<String>,
    }

    impl FromStr for CommandPrefix {
        type Err = ScopeError;

        fn from_str(value: &str) -> Result<Self, Self::Err> {
            let tokens = value
                .split_whitespace()
                .map(String::from)
                .collect::<Vec<_>>();
            let Some(program) = tokens.first() else {
                return Err(ScopeError::unknown(format!(
                    "{value:?}: command prefixes must contain at least one token"
                )));
            };
            if program.contains(['/', '\\']) {
                return Err(ScopeError::unknown(format!(
                    "{value:?}: first token {program:?} contains a path separator; program paths are resolved when spawning"
                )));
            }
            Ok(Self { tokens })
        }
    }

    impl ScopeRepr for CommandPrefix {
        fn canonical(&self) -> String {
            self.tokens.join(" ")
        }
    }

    impl Scope for CommandPrefix {
        fn contains(&self, inner: &Self) -> bool {
            inner.tokens.starts_with(&self.tokens)
        }
    }

    /// Spawn a process whose argv starts with a granted prefix.
    pub const RUN: ScopedPermission<CommandPrefix> = ScopedPermission::new("run");
}

#[cfg(test)]
mod tests {
    use core::str::FromStr;
    use lockgate_policy::{Scope, ScopeRepr, check_scope_laws};

    use super::exec::CommandPrefix;

    fn prefix(value: &str) -> CommandPrefix {
        CommandPrefix::from_str(value).unwrap()
    }

    #[test]
    fn command_prefix_samples_obey_the_scope_laws() {
        check_scope_laws([
            prefix("cargo"),
            prefix("git"),
            prefix("git commit"),
            prefix("git diff"),
            prefix("sh scripts/foo.sh"),
        ])
        .unwrap();
    }

    #[test]
    fn containment_follows_argv_prefixes() {
        let cargo = prefix("cargo");
        let git = prefix("git");
        let git_commit = prefix("git commit");
        let git_diff = prefix("git diff");

        assert!(git.contains(&git_commit));
        assert!(!git_commit.contains(&git));
        assert!(!git_commit.contains(&git_diff));
        assert!(!git_diff.contains(&git_commit));
        assert!(git_commit.contains(&git_commit));
        assert!(!git.contains(&cargo));
        assert!(!cargo.contains(&git));
    }

    #[test]
    fn parsing_normalizes_whitespace_and_round_trips() {
        let normalized = prefix("  git   commit ");
        assert_eq!(normalized.canonical(), "git commit");

        let canonical = prefix("sh scripts/foo.sh");
        assert_eq!(
            CommandPrefix::from_str(&canonical.canonical()).unwrap(),
            canonical
        );
        assert_eq!(canonical.canonical(), "sh scripts/foo.sh");
    }

    #[test]
    fn parsing_rejects_empty_and_path_shaped_programs() {
        for invalid in ["", "   ", "/usr/bin/git", "./x foo", r"\git"] {
            assert!(
                CommandPrefix::from_str(invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
    }
}
