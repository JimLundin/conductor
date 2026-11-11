// session.rs:425-432
ClaudeEvent::SessionStarted { session_id } => match self
    .session_id
    .replace(session_id.clone())
    .map(|seen| seen.0.cmp(&session_id.0))
{
    Some(Ordering::Equal) => News::Repeat,
    Some(Ordering::Less | Ordering::Greater) | None => News::Fresh,
},
// s2.rs:198-203
match event_turn(&event).map(|got| (got.0.cmp(&turn.id.0), got)) {
    Some((Ordering::Less | Ordering::Greater, got)) => Err(anyhow::Error::msg(
        format!("event for turn {} during turn {}", got.0, turn.id.0),
    )),
    Some((Ordering::Equal, _)) | None => Ok(()),
}?;
// s2.rs:554-560
match (
    resumed.id.0.cmp(&a.id.0),
    s.turn.text.find("a.txt"),
    &s.turn.outcome,
) {
    (Ordering::Equal, Some(_), Some(TurnOutcome::Completed)) => Verdict::Pass,
    _ => Verdict::Fail,
}
// codex_replay.rs:204-209
(ThreadEvent::TurnCompleted { turn: done, .. }, _) => {
    match done.0.cmp(&turn.0) {
        Ordering::Equal => Progress::Done,
        Ordering::Less | Ordering::Greater => Progress::Playing,
    }
}
// s1.rs:325-336
match (
    t.permissions.cmp(&Vec::from([String::from("Write")])),
    presence(&a),
    t.result(),
) {
    (
        std::cmp::Ordering::Equal,
        Presence::Present,
        Some(("success", Outcome::Succeeded)),
    ) => Verdict::Pass,
    _ => Verdict::Fail,
}
