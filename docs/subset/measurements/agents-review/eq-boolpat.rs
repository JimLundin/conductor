// session.rs:425-432 with `true`/`false` patterns on a std `bool`
ClaudeEvent::SessionStarted { session_id } => match self
    .session_id
    .replace(session_id.clone())
    .map(|seen| seen.eq(session_id))
{
    Some(true) => News::Repeat,
    Some(false) | None => News::Fresh,
},
// s2.rs:198-203
match event_turn(&event).map(|got| (got.eq(&turn.id), got)) {
    Some((false, got)) => Err(anyhow::Error::msg(format!(
        "event for turn {} during turn {}",
        got.0, turn.id.0
    ))),
    Some((true, _)) | None => Ok(()),
}?;
// s2.rs:554-560
match (
    resumed.id.eq(&a.id),
    s.turn.text.find("a.txt"),
    &s.turn.outcome,
) {
    (true, Some(_), Some(TurnOutcome::Completed)) => Verdict::Pass,
    _ => Verdict::Fail,
}
// codex_replay.rs:204-209
(ThreadEvent::TurnCompleted { turn: done, .. }, _) => match done.eq(turn) {
    true => Progress::Done,
    false => Progress::Playing,
},
// s1.rs:325-336
match (
    t.permissions.eq(&Vec::from([String::from("Write")])),
    presence(&a),
    t.result(),
) {
    (true, Presence::Present, Some(("success", Outcome::Succeeded))) => Verdict::Pass,
    _ => Verdict::Fail,
}
