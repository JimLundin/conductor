use crate::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Var {
    Set { name: String, value: String },
    Remove(String),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeKind {
    User,
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
