//! Host ownership and commands for the session shown.

use alloc::collections::BTreeMap;

use client::host_view::{Movement, SessionView};
use proto::movement::Phase;
use proto::{CommandBody, CommandResult, SessionId};

use crate::host::{Host, HostId};
use crate::prelude::*;
use crate::{At, Effect, ReplyTo};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveIntent {
    To(HostId),
    Retry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveTarget {
    pub id: HostId,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MovePanel {
    Hidden,
    Offer(Vec<MoveTarget>),
    Starting,
    Moving {
        text: String,
        can_retry: bool,
    },
    Failed {
        text: String,
        can_retry: bool,
        targets: Vec<MoveTarget>,
    },
    Moved(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Draft {
    pub pending: bool,
    pub uncertain: bool,
}

impl Draft {
    pub fn command(
        &mut self,
        intent: MoveIntent,
        session: SessionId,
        view: &SessionView,
        hosts: &BTreeMap<HostId, Host>,
        at: At,
    ) -> Option<Effect> {
        if self.pending {
            return None;
        }
        let source = hosts.get(&at.host)?;
        let (to, retry) = match intent {
            MoveIntent::To(to)
                if view.can_start_move()
                    && allowed(hosts, source, at.host, to) =>
            {
                (to.0, None)
            }
            MoveIntent::Retry if retryable(view, source, self.uncertain) => {
                let (to, retry) = view.movement.transfer()?;
                (to, Some(retry))
            }
            _ => return None,
        };
        self.pending = true;
        self.uncertain = false;
        Some(at.send(
            CommandBody::MoveSession { session, to, retry },
            ReplyTo::Move,
        ))
    }

    pub const fn replied(&mut self, answer: &Result<CommandResult, String>) {
        if !matches!(answer, Ok(CommandResult::Accepted)) {
            self.pending = false;
            self.uncertain = answer.is_err();
        }
    }
}

pub fn panel(
    hosts: &BTreeMap<HostId, Host>,
    here: HostId,
    source: &Host,
    session: &SessionView,
    draft: &Draft,
) -> MovePanel {
    if draft.pending {
        return MovePanel::Starting;
    }
    let label = |to| name(hosts, HostId(to));
    match &session.movement {
        Movement::Moving { to, phase, .. } => MovePanel::Moving {
            text: format!(
                "{} for the move to {}.",
                phase_title(*phase),
                label(*to)
            ),
            can_retry: retryable(session, source, draft.uncertain),
        },
        Movement::Failed { to, reason, .. } => MovePanel::Failed {
            text: format!(
                "Could not finish moving to {}. {reason}",
                label(*to)
            ),
            can_retry: retryable(session, source, draft.uncertain),
            targets: candidates(hosts, here, source, session)
                .into_iter()
                .filter(|target| target.id.0 != *to)
                .collect(),
        },
        Movement::Moved { to, .. } => MovePanel::Moved(format!(
            "Moved to {}. The session will appear there once its history is available.",
            label(*to)
        )),
        Movement::Owned => {
            let targets = candidates(hosts, here, source, session);
            if targets.is_empty() {
                MovePanel::Hidden
            } else {
                MovePanel::Offer(targets)
            }
        }
    }
}

fn candidates(
    hosts: &BTreeMap<HostId, Host>,
    here: HostId,
    source: &Host,
    session: &SessionView,
) -> Vec<MoveTarget> {
    if !session.can_start_move() {
        return Vec::new();
    }
    let mut targets = hosts
        .iter()
        .filter(|(to, _)| allowed(hosts, source, here, **to))
        .map(|(&id, host)| MoveTarget {
            id,
            name: host.name(),
        })
        .collect::<Vec<_>>();
    targets.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    targets
}

fn allowed(
    hosts: &BTreeMap<HostId, Host>,
    source: &Host,
    here: HostId,
    to: HostId,
) -> bool {
    to != here
        && source.fold.live()
        && source.fold.move_targets.contains(&to.0)
        && hosts.get(&to).is_some_and(|host| host.fold.live())
}

fn retryable(session: &SessionView, source: &Host, uncertain: bool) -> bool {
    source.fold.live()
        && match session.movement {
            Movement::Failed {
                resumable: true, ..
            } => session.can_start_move(),
            Movement::Failed {
                resumable: false, ..
            } => true,
            Movement::Moving { .. } => uncertain,
            _ => false,
        }
}

fn name(hosts: &BTreeMap<HostId, Host>, host: HostId) -> String {
    hosts.get(&host).map_or_else(|| host.key(), Host::name)
}

const fn phase_title(phase: Phase) -> &'static str {
    match phase {
        Phase::Settling => "Preparing the session",
        Phase::CheckingTarget => "Checking the destination",
        Phase::SendingWorkspace => "Sending the worktree",
        Phase::SendingHistory => "Sending the conversation and checkpoints",
        Phase::Committing => "Confirming the receipt",
    }
}

pub fn visible(
    hosts: &BTreeMap<HostId, Host>,
    host: HostId,
    id: SessionId,
    session: &SessionView,
) -> bool {
    session.history_available() && owner(hosts, host, id) == host
}

pub fn owner(
    hosts: &BTreeMap<HostId, Host>,
    from: HostId,
    session: SessionId,
) -> HostId {
    if hosts.len() < 2 {
        return from;
    }
    let mut node = from;
    for _ in hosts {
        let Some(next) = successor(hosts, node, session) else {
            return node;
        };
        node = next;
    }
    from
}

fn successor(
    hosts: &BTreeMap<HostId, Host>,
    from: HostId,
    session: SessionId,
) -> Option<HostId> {
    let source = hosts.get(&from)?;
    let view = source.fold.view.sessions.get(&session)?;
    let transfer = view.movement.transfer();
    hosts.iter().find_map(|(&target, host)| {
        let arrival = host.fold.view.sessions.get(&session)?.arrival?;
        // The destination can publish its receipt before the source feed
        // reaches the move. Only an earlier view of that same epoch qualifies.
        let matched = transfer.map_or_else(
            || view.activity <= arrival.source_last_seq,
            |expected| expected == (target.0, arrival.move_id),
        );
        (target != from
            && arrival.ready
            && arrival.from == from.0
            && source.fold.epoch == Some(arrival.source_epoch)
            && matched)
            .then_some(target)
    })
}
