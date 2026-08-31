//! The [`ScopedResource`] half of the exec capability: how a concrete spawn
//! request reports the scope memberships (witnesses, in
//! [`lockgate_policy::Scope`] vocabulary) that admission grants are checked
//! against.

use std::str::FromStr;
use std::string::String;
use std::vec::Vec;

use lockgate::{PluginSubject, ScopedResource};

use crate::{MAX_WITNESSES, exec::CommandPrefix};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandTarget {
    pub program: String,
    pub args: Vec<String>,
}

impl CommandTarget {
    fn witnesses(&self) -> Vec<CommandPrefix> {
        let mut canonical = String::new();
        let mut witnesses = Vec::new();

        // Witness count must stay bounded independently of argv length.
        for token in std::iter::once(&self.program)
            .chain(self.args.iter())
            .take(MAX_WITNESSES)
        {
            if token.is_empty() || token.chars().any(char::is_whitespace) {
                break;
            }
            if !canonical.is_empty() {
                canonical.push(' ');
            }
            canonical.push_str(token);
            let Ok(prefix) = CommandPrefix::from_str(&canonical) else {
                break;
            };
            witnesses.push(prefix);
        }
        witnesses
    }
}

impl ScopedResource<CommandPrefix> for CommandTarget {
    fn scopes_for(&self, _subject: &PluginSubject<'_>) -> Vec<CommandPrefix> {
        self.witnesses()
    }
}

#[cfg(test)]
mod tests {
    use std::format;
    use std::string::String;
    use std::vec;
    use std::vec::Vec;

    use lockgate_policy::ScopeRepr;

    use super::{CommandTarget, MAX_WITNESSES};

    fn canonical_witnesses(target: &CommandTarget) -> Vec<String> {
        target
            .witnesses()
            .iter()
            .map(ScopeRepr::canonical)
            .collect()
    }

    #[test]
    fn reports_prefixes_until_an_argument_contains_whitespace() {
        let target = CommandTarget {
            program: String::from("git"),
            args: vec![
                String::from("commit"),
                String::from("-m"),
                String::from("x y"),
            ],
        };

        assert_eq!(
            canonical_witnesses(&target),
            ["git", "git commit", "git commit -m"]
        );
    }

    #[test]
    fn path_shaped_program_reports_no_witnesses() {
        let target = CommandTarget {
            program: String::from("/usr/bin/git"),
            args: vec![String::from("status")],
        };
        assert!(target.witnesses().is_empty());
    }

    #[test]
    fn caps_witnesses_at_sixteen_tokens() {
        let target = CommandTarget {
            program: String::from("tool"),
            args: (1..=20).map(|index| format!("arg{index}")).collect(),
        };
        let witnesses = canonical_witnesses(&target);

        assert_eq!(witnesses.len(), MAX_WITNESSES);
        assert_eq!(
            witnesses.last().unwrap(),
            "tool arg1 arg2 arg3 arg4 arg5 arg6 arg7 arg8 arg9 arg10 arg11 arg12 arg13 arg14 arg15"
        );
    }

    #[test]
    fn empty_argument_stops_witness_reporting() {
        let target = CommandTarget {
            program: String::from("git"),
            args: vec![String::from("commit"), String::new(), String::from("-m")],
        };
        assert_eq!(canonical_witnesses(&target), ["git", "git commit"]);
    }
}
