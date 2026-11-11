// claude/protocol.rs:172-175 and 230-233 with `true`/`false` patterns on a library `bool`
outcome: match msg.is_error {
    true => Outcome::Failed,
    false => Outcome::Succeeded,
},
outcome: match t.is_error {
    Some(true) => Outcome::Failed,
    Some(false) | None => Outcome::Succeeded,
},
