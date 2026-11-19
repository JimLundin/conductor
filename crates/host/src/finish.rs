//! Finishing a session on an issue (docs/home.md §4).
//!
//! Whether it can, the summary its agent gives, how its branch lands on
//! the issue's, and what the log says of it. The shell asks the agent,
//! runs `git` and writes the issue's note.

use alloc::format;

use proto::{
    CommandResult, Event, EventKind, FinishState, Finished, HostSeq,
    InstanceId, IssueId, Rejection, SessionId, Workspace,
};

use crate::prelude::*;
use crate::workspace::kept;

/// What the agent of a session being finished is asked, as its last turn.
pub const SUMMARY: &str = "Conductor is finishing this session. In a few sentences, sum up what you did on the issue and what is left, as a note for whoever picks it up next. Make no more changes.";

/// Why the agent of a finished session stops.
pub const FINISHED: &str = "the session was finished";

/// A session that can finish, as its log has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finishing {
    pub issue: IssueId,
    pub instance: InstanceId,
    pub workspace: Workspace,
    /// Its agent runs, so it is asked for a summary first.
    pub running: bool,
}

/// Whether `session` can finish after `history`: it is on an issue, has a
/// workspace, isn't removed, runs no turn, and has done something since
/// it last finished. A session still starting counts as running.
///
/// # Errors
///
/// Why it can't.
pub fn finishing(
    history: &[Event],
    session: SessionId,
) -> Result<Finishing, Rejection> {
    let mut created = None;
    let mut issue = None;
    let mut workspace = None;
    let mut removed = false;
    let mut running = false;
    let mut turning = false;
    let mut finish = FinishState::Ready;
    for event in history.iter().filter(|event| event.session == session) {
        match &event.kind {
            EventKind::SessionCreated { instance, .. } => {
                created = Some(instance.clone());
                running = true;
            }
            EventKind::OnIssue { issue: on } => issue = Some(*on),
            EventKind::WorkspaceCreated { workspace: made } => {
                workspace = Some(made.clone());
            }
            EventKind::SessionRemoved { .. } => removed = true,
            EventKind::SessionReady => running = true,
            EventKind::UserMessage { .. } => turning = true,
            EventKind::TurnCompleted { .. } => turning = false,
            EventKind::SessionClosed { .. } => {
                (running, turning) = (false, false);
            }
            _ => {}
        }
        finish = finish.after(&event.kind);
    }
    let instance = created.ok_or(Rejection::UnknownSession)?;
    let refused = [
        (removed, Rejection::SessionRemoved),
        (turning, Rejection::TurnInProgress),
        (finish != FinishState::Ready, Rejection::AlreadyFinished),
    ];
    if let Some((_, rejection)) = refused.into_iter().find(|(is, _)| *is) {
        return Err(rejection);
    }
    Ok(Finishing {
        issue: issue.ok_or(Rejection::NotOnIssue)?,
        instance,
        workspace: workspace.ok_or_else(|| Rejection::WorkspaceFailure {
            reason: String::from("the session has no workspace to land"),
        })?,
        running,
    })
}

/// The summary for the issue's notes: the session's last reply after
/// `after`, the seq the summary turn started after, or its last reply of
/// all when its agent had stopped.
#[must_use]
pub fn summary(
    events: &[Event],
    session: SessionId,
    after: HostSeq,
) -> Option<String> {
    events
        .iter()
        .filter(|event| event.session == session && event.seq > after)
        .filter_map(|event| match &event.kind {
            EventKind::AssistantMessage { text } => Some(text.clone()),
            _ => None,
        })
        .next_back()
        .filter(|text| !text.trim().is_empty())
}

/// Whether the summary turn has ended, by the event the log stored.
#[must_use]
pub fn turn_ended(event: &Event, session: SessionId) -> bool {
    event.session == session
        && matches!(
            event.kind,
            EventKind::TurnCompleted { .. } | EventKind::SessionClosed { .. }
        )
}

/// What `git merge-tree --write-tree --name-only --no-messages` gave: the
/// merged tree, or why there is none.
///
/// # Errors
///
/// The files that conflict, or what Git said when it failed otherwise.
pub fn merged(
    success: bool,
    output: &str,
    error: &str,
) -> Result<String, String> {
    let mut lines = output.lines();
    let tree = lines.next().unwrap_or_default();
    match (success, error.trim()) {
        (true, _) => Ok(String::from(tree)),
        (false, "") => Err(format!(
            "the session's branch conflicts with the issue's in {}; merge them by hand",
            lines.collect::<Vec<&str>>().join(", ")
        )),
        (false, said) => {
            Err(format!("the branches couldn't be merged: {said}"))
        }
    }
}

/// What the shell found of the session's and the issue's branches in the
/// checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branches {
    /// The commits on the session's branch the issue's lacks.
    pub ahead: u64,
    /// The issue's branch is an ancestor of the session's.
    pub fast_forward: bool,
    /// Where the issue's branch is checked out, if it is.
    pub checked_out: Option<String>,
}

/// How the session's branch lands on the issue's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Landing {
    /// The issue's branch has it all already.
    Nothing,
    FastForward,
    /// A merge commit of both, made without a worktree.
    Merge,
    Refuse(String),
}

impl Branches {
    #[must_use]
    pub fn landing(self) -> Landing {
        match (self.checked_out, self.ahead, self.fast_forward) {
            (Some(path), _, _) => Landing::Refuse(format!(
                "the issue's branch is checked out at {path}, so it can't move under it; land the session's branch there"
            )),
            (None, 0, _) => Landing::Nothing,
            (None, _, true) => Landing::FastForward,
            (None, _, false) => Landing::Merge,
        }
    }
}

/// Where `branch`, a full ref, is checked out, from `git worktree list
/// --porcelain`.
#[must_use]
pub fn checked_out(porcelain: &str, branch: &str) -> Option<String> {
    let wanted = format!("branch {branch}");
    porcelain.split("\n\n").find_map(|record| {
        let mut lines = record.lines();
        let path = lines.next()?.strip_prefix("worktree ")?;
        lines.any(|line| line == wanted).then(|| String::from(path))
    })
}

/// The message of the merge commit that lands `session` on `issue`.
#[must_use]
pub fn merge_message(issue: &str, session: &str) -> String {
    format!("Land {session} on {issue}")
}

/// Who signs the notes a session writes on its issue: its instance.
#[must_use]
pub fn signer(instance: &InstanceId) -> String {
    match &instance.account {
        proto::Account::Own => String::from(instance.provider.name()),
        proto::Account::Named(account) => {
            format!("{} · {account}", instance.provider.name())
        }
    }
}

/// What finishing did: the commits that landed, or why none did, and the
/// push of the issue's branch.
#[must_use]
pub fn finished(
    landed: Result<u64, String>,
    pushed: Option<(String, Result<(), String>)>,
) -> Finished {
    match landed {
        Ok(commits) => Finished::Landed {
            commits,
            kept: kept(pushed),
        },
        Err(reason) => Finished::Failed { reason },
    }
}

/// The commits that landed, for moving the issue on.
#[must_use]
pub const fn landed(finished: &Finished) -> u64 {
    match finished {
        Finished::Landed { commits, .. } => *commits,
        Finished::Failed { .. } => 0,
    }
}

/// How long the agent has to sum up before finishing gives up, in
/// seconds.
pub const PATIENCE: u64 = 600;

/// Why finishing landed nothing when the summary turn didn't end in time.
pub const IMPATIENT: &str =
    "the agent didn't sum up within 10 minutes, so it was interrupted";

/// Why finishing landed nothing when the agent wasn't asked or stopped:
/// a turn got in first.
#[must_use]
pub fn interrupted(
    what: &str,
    answer: Option<&CommandResult>,
) -> Option<String> {
    match answer {
        Some(CommandResult::Accepted) => None,
        Some(CommandResult::Rejected(rejection)) => {
            Some(format!("{what}: {}", rejection.label()))
        }
        Some(other) => Some(format!("{what}: the session answered {other:?}")),
        None => Some(format!("{what}: the session stopped")),
    }
}

/// Whether to wait for the summary turn to end: its message reached the
/// agent, and the events `seen` since don't end it.
#[must_use]
pub fn waits(
    answer: Option<&CommandResult>,
    seen: &[Event],
    session: SessionId,
) -> bool {
    matches!(answer, Some(CommandResult::Accepted))
        && !seen.iter().any(|event| turn_ended(event, session))
}

#[cfg(test)]
mod tests;
