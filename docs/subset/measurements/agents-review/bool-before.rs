// claude/protocol.rs:172-175 and 230-233
outcome: match msg.is_error.then_some(Outcome::Failed) {
    Some(failed) => failed,
    None => Outcome::Succeeded,
},
outcome: match t.is_error.unwrap_or(false).then_some(Outcome::Failed) {
    Some(failed) => failed,
    None => Outcome::Succeeded,
},
