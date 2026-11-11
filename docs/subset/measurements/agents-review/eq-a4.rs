// session.rs:425-432, the bootstrap review's A4 form: `.eq(..)` with `.then_some(..)`
ClaudeEvent::SessionStarted { session_id } => match self
    .session_id
    .replace(session_id.clone())
    .and_then(|seen| seen.eq(session_id).then_some(News::Repeat))
{
    Some(repeat) => repeat,
    None => News::Fresh,
},
// s2.rs:198-203
match event_turn(&event).and_then(|got| got.ne(&turn.id).then_some(got)) {
    Some(got) => Err(anyhow::Error::msg(format!(
        "event for turn {} during turn {}",
        got.0, turn.id.0
    ))),
    None => Ok(()),
}?;
// s2.rs:554-560
match (
    resumed.id.eq(&a.id).then_some(&resumed.id),
    s.turn.text.find("a.txt"),
    &s.turn.outcome,
) {
    (Some(_), Some(_), Some(TurnOutcome::Completed)) => Verdict::Pass,
    _ => Verdict::Fail,
}
// codex_replay.rs:204-209
(ThreadEvent::TurnCompleted { turn: done, .. }, _) => {
    match done.eq(turn).then_some(Progress::Done) {
        Some(finished) => finished,
        None => Progress::Playing,
    }
}
// s1.rs:325-336
match (
    t.permissions
        .eq(&Vec::from([String::from("Write")]))
        .then_some(&t.permissions),
    presence(&a),
    t.result(),
) {
    (Some(_), Presence::Present, Some(("success", Outcome::Succeeded))) => Verdict::Pass,
    _ => Verdict::Fail,
}
