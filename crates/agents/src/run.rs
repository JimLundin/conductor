//! A CLI run as data: what its process starts with. The shell turns it into
//! a command.

use crate::prelude::*;

/// One change to the environment a CLI inherits. The changes apply in
/// order, so a later one for the same variable wins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Var {
    Set {
        name: String,
        value: String,
    },
    Remove(String),
    /// Set to the CLI's private home directory.
    Home(String),
}

impl Var {
    #[must_use]
    pub fn set(name: &str, value: &str) -> Self {
        Self::Set {
            name: String::from(name),
            value: String::from(value),
        }
    }

    #[must_use]
    pub fn remove(name: &str) -> Self {
        Self::Remove(String::from(name))
    }
}

/// Whose home a CLI keeps its config, credentials and transcripts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeKind {
    /// The user's own.
    User,
    /// A directory of the host's own, so the user's homes stay untouched.
    Private,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Invocation {
    pub env: Vec<Var>,
    pub args: Vec<String>,
}

impl Invocation {
    #[must_use]
    pub fn of(env: Vec<Var>, args: &[&str]) -> Self {
        Self {
            env,
            args: args.iter().copied().map(String::from).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Invocation, Var};

    #[test]
    fn an_invocation_keeps_its_environment_and_arguments_in_order() {
        assert_eq!(
            Invocation::of(Vec::from([Var::remove("A")]), &["-p", "x"]),
            Invocation {
                env: Vec::from([Var::remove("A")]),
                args: Vec::from([String::from("-p"), String::from("x")]),
            }
        );
    }
}
