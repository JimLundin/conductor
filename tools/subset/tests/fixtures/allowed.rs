//! Uses every core construct; `subset check` reports nothing here.

mod nested {}

use std::collections::BTreeMap;

const LIMIT: u32 = 3;

/// A named-field struct.
#[derive(Debug, Clone)]
struct Session {
    id: SessionId,
    name: String,
}

struct SessionId(u64);

enum State {
    Idle,
    Running(SessionId),
    Done { code: u32 },
}

impl Session {
    fn label(&self) -> String {
        format!("{}-{}", self.name, LIMIT)
    }

    fn id(&self) -> &SessionId {
        &self.id
    }
}

fn describe(state: &State) -> Option<&str> {
    let text = match state {
        State::Idle => "idle",
        State::Running(_) => "running",
        State::Done { code: 0 } | State::Done { code: 1 } => "done",
        State::Done { code } => describe_code(code)?,
    };
    Some({ (text) })
}

fn describe_code(code: &u32) -> Option<&str> {
    BTreeMap::new().get(code).copied()
}

fn names(sessions: &Vec<Session>) -> Vec<String> {
    sessions.iter().map(|s| s.label()).collect()
}

fn build() -> Session {
    Session {
        id: SessionId(7),
        name: String::from("x"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn label() {
        assert_eq!(super::build().label(), "x-3");
    }
}
