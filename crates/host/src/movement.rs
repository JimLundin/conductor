//! Refs carried in a session move.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::ToString;

use proto::fleet::NodeId;
use proto::movement::{MoveId, Phase};
use proto::{
    CloseReason, Epoch, Event, EventKind, HostSeq, InstanceId, SessionId,
    Workspace,
};
use serde::{Deserialize, Serialize};

use crate::prelude::*;
use crate::workspace::{GitError, branch};

/// Only the session branch and its numeric checkpoint refs may arrive in a bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refs {
    pub tip: String,
    pub checkpoints: BTreeMap<u64, String>,
}

impl Refs {
    /// Interpret the refs advertised by Git.
    ///
    /// # Errors
    /// A ref, object ID, or checkpoint name is invalid or occurs twice.
    pub fn listed(session: SessionId, output: &str) -> Result<Self, GitError> {
        let head = head(session);
        let prefix = checkpoints(session);
        let mut tip = None;
        let mut checkpoints = BTreeMap::new();
        for line in output.lines() {
            let (object, name) = line.split_once(' ').ok_or_else(invalid)?;
            if !is_object(object) {
                return Err(invalid());
            }
            if name == head {
                if tip.replace(String::from(object)).is_some() {
                    return Err(invalid());
                }
            } else {
                let id = checkpoint_id(name, &prefix).ok_or_else(invalid)?;
                if checkpoints.insert(id, String::from(object)).is_some() {
                    return Err(invalid());
                }
            }
        }
        Ok(Self {
            tip: tip.ok_or_else(invalid)?,
            checkpoints,
        })
    }

    /// Checks that a thin bundle advertises only exact refs in its manifest.
    ///
    /// # Errors
    /// The manifest contains an invalid object name or changed ref.
    pub fn validate_bundle(&self, listed: &Self) -> Result<(), GitError> {
        if self.tip == listed.tip
            && self.checkpoints.values().all(|commit| is_object(commit))
            && listed
                .checkpoints
                .iter()
                .all(|(id, commit)| self.checkpoints.get(id) == Some(commit))
        {
            Ok(())
        } else {
            Err(invalid())
        }
    }

    /// Checkpoint commits omitted from a thin bundle must already be on the target.
    #[must_use]
    pub fn omitted<'a>(
        &'a self,
        listed: &Self,
        session: SessionId,
    ) -> Vec<(String, &'a str)> {
        let prefix = checkpoints(session);
        self.checkpoints
            .iter()
            .filter(|(id, _)| !listed.checkpoints.contains_key(id))
            .map(|(id, commit)| (format!("{prefix}{id}"), commit.as_str()))
            .collect()
    }

    /// The refs and exact commits the bundle must install.
    #[must_use]
    pub fn mappings(&self, session: SessionId) -> Vec<(String, &str)> {
        let mut refs = Vec::with_capacity(self.checkpoints.len() + 1);
        refs.push((head(session), self.tip.as_str()));
        let prefix = checkpoints(session);
        refs.extend(
            self.checkpoints
                .iter()
                .map(|(id, commit)| (format!("{prefix}{id}"), commit.as_str())),
        );
        refs
    }
}

fn invalid() -> GitError {
    GitError::failed("session move", "unexpected refs in session bundle")
}

fn checkpoint_id(name: &str, prefix: &str) -> Option<u64> {
    let suffix = name.strip_prefix(prefix)?;
    let number = suffix.parse::<u64>().ok()?;
    (number.to_string() == suffix).then_some(number)
}

#[must_use]
pub fn is_object(object: &str) -> bool {
    matches!(object.len(), 40 | 64)
        && object
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

#[must_use]
pub fn head(session: SessionId) -> String {
    format!("refs/heads/{}", branch(session))
}

#[must_use]
pub fn checkpoints(session: SessionId) -> String {
    format!("refs/conductor/{}/", ulid::Ulid::from(session.0))
}

/// The branch is fetched here while an incoming move prepares its worktree.
#[must_use]
pub fn incoming(session: SessionId) -> String {
    format!("refs/conductor-moves/{}", ulid::Ulid::from(session.0))
}

/// The source remains locked once a transfer may have reached the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ownership {
    Local,
    Moving {
        to: NodeId,
        id: MoveId,
        phase: Phase,
    },
    Released {
        to: NodeId,
        id: MoveId,
    },
}

impl Ownership {
    #[must_use]
    pub fn replay(history: &[Event]) -> Self {
        history
            .iter()
            .fold(Self::Local, |state, event| match &event.kind {
                EventKind::SessionMoving { to, move_id, phase } => {
                    Self::Moving {
                        to: *to,
                        id: *move_id,
                        phase: *phase,
                    }
                }
                EventKind::SessionMoved { to, move_id } => Self::Released {
                    to: *to,
                    id: *move_id,
                },
                EventKind::SessionArrived { .. }
                | EventKind::SessionMoveFailed {
                    resumable: true, ..
                } => Self::Local,
                _ => state,
            })
    }
}

/// Rebuilds the arriving session using destination paths and host cursors.
pub struct History<'a> {
    source: &'a [Event],
    local: BTreeMap<HostSeq, HostSeq>,
    destination: &'a InstanceId,
    workspace: &'a Workspace,
}

impl<'a> History<'a> {
    #[must_use]
    pub const fn prepare(
        source: &'a [Event],
        destination: &'a InstanceId,
        workspace: &'a Workspace,
    ) -> Self {
        Self {
            source,
            local: BTreeMap::new(),
            destination,
            workspace,
        }
    }

    pub fn events(&self) -> impl Iterator<Item = &'a Event> + use<'a> {
        self.source.iter().filter(|event| portable(&event.kind))
    }

    pub fn recorded(&mut self, source: HostSeq, local: HostSeq) {
        self.local.insert(source, local);
    }

    /// Copies a historical event, remapping host-local sequence references.
    ///
    /// # Errors
    /// A checkpoint refers to an absent or later turn start.
    pub fn localize(&self, event: &Event) -> Result<EventKind, GitError> {
        let mut kind = event.kind.clone();
        match &mut kind {
            EventKind::SessionCreated {
                instance,
                directory,
                ..
            } => {
                instance.clone_from(self.destination);
                directory.clone_from(&self.workspace.original);
            }
            EventKind::WorkspaceCreated { workspace } => {
                workspace.clone_from(self.workspace);
            }
            EventKind::CheckpointCreated { checkpoint } => {
                checkpoint.turn_start = self.sequence(checkpoint.turn_start)?;
            }
            EventKind::NativeTurnStarted { turn_start, .. } => {
                *turn_start = self.sequence(*turn_start)?;
            }
            _ => {}
        }
        Ok(kind)
    }

    fn sequence(
        &self,
        sequence: Option<HostSeq>,
    ) -> Result<Option<HostSeq>, GitError> {
        sequence
            .map(|cursor| {
                self.local.get(&cursor).copied().ok_or_else(|| {
                    GitError::failed(
                        "session move",
                        "missing turn start in transferred history",
                    )
                })
            })
            .transpose()
    }
}

const fn portable(kind: &EventKind) -> bool {
    !matches!(
        kind,
        EventKind::SessionMoving { .. }
            | EventKind::SessionMoveFailed { .. }
            | EventKind::SessionMoved { .. }
            | EventKind::SessionArrived { .. }
            | EventKind::SessionArrivalReady { .. }
            | EventKind::SessionMoveNoticeSent
            | EventKind::AssistantDelta { .. }
            | EventKind::ReasoningDelta { .. }
    )
}

/// One idempotent import from a specific certified source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Incoming {
    pub from: NodeId,
    pub id: MoveId,
    pub session: SessionId,
}

impl Incoming {
    #[must_use]
    pub fn receipt(self, history: &[Event]) -> bool {
        history.iter().any(|event| {
            event.session == self.session
                && matches!(&event.kind,
                EventKind::SessionArrived { from, move_id, .. }
                    if from == &self.from && move_id == &self.id)
        })
    }
}

/// Files are already installed when this batch is committed.
pub struct Arrival {
    pub incoming: Incoming,
    pub source_epoch: Epoch,
    pub source_last_seq: HostSeq,
    pub destination: InstanceId,
    pub workspace: Workspace,
    pub history: Vec<Event>,
    pub conversation: Option<String>,
}

impl Arrival {
    /// Validate the whole event batch before it can publish any events.
    ///
    /// # Errors
    /// Invalid session history or missing cursor references.
    pub fn events(&self, after: HostSeq) -> Result<Vec<Event>, GitError> {
        let session = self.incoming.session;
        let mut seq = after.next();
        let mut result = Vec::with_capacity(self.history.len() + 3);
        result.push(Event {
            seq,
            session,
            kind: EventKind::SessionArrived {
                from: self.incoming.from,
                move_id: self.incoming.id,
                source_epoch: self.source_epoch,
                source_last_seq: self.source_last_seq,
            },
        });
        if self
            .history
            .last()
            .is_some_and(|event| event.seq > self.source_last_seq)
        {
            return Err(GitError::failed(
                "session move",
                "the source cursor excludes transferred events",
            ));
        }
        let portable = localized(
            session,
            &self.history,
            &self.destination,
            &self.workspace,
            seq,
        )?;
        seq = portable.last().map_or(seq, |event| event.seq);
        result.extend(portable);
        seq = seq.next();
        result.push(Event {
            seq,
            session,
            kind: EventKind::SessionClosed {
                reason: CloseReason::Stopped {
                    reason: String::from("the session arrived and can resume"),
                },
            },
        });
        result.push(Event {
            seq: seq.next(),
            session,
            kind: EventKind::SessionArrivalReady {
                move_id: self.incoming.id,
            },
        });
        Ok(result)
    }

    /// The new owner must carry every durable event from the prior local ownership.
    ///
    /// # Errors
    /// The incoming history lost or changed part of the previous history.
    pub fn confirm_successor(&self, local: &[Event]) -> Result<(), GitError> {
        is_successor(
            self.incoming.session,
            local,
            &self.history,
            &self.destination,
            &self.workspace,
        )
    }
}

/// Checks lineage before a returning move changes the old checkout.
///
/// # Errors
/// The transferred history omits or changes earlier events.
pub fn is_successor(
    session: SessionId,
    local: &[Event],
    next: &[Event],
    destination: &InstanceId,
    workspace: &Workspace,
) -> Result<(), GitError> {
    let current =
        localized(session, local, destination, workspace, HostSeq(0))?;
    let incoming =
        localized(session, next, destination, workspace, HostSeq(0))?;
    if incoming.starts_with(&current) {
        Ok(())
    } else {
        Err(GitError::failed(
            "session move",
            "the incoming history does not extend this host's released copy",
        ))
    }
}

fn localized(
    session: SessionId,
    events: &[Event],
    destination: &InstanceId,
    workspace: &Workspace,
    after: HostSeq,
) -> Result<Vec<Event>, GitError> {
    let mut history = History::prepare(events, destination, workspace);
    let mut seq = after;
    let mut old = HostSeq(0);
    let mut result = Vec::new();
    for source in history.events() {
        if source.session != session || source.seq <= old {
            return Err(GitError::failed(
                "session move",
                "invalid event order in transferred history",
            ));
        }
        seq = seq.next();
        let kind = history.localize(source)?;
        result.push(Event { seq, session, kind });
        history.recorded(source.seq, seq);
        old = source.seq;
    }
    Ok(result)
}

#[must_use]
pub fn compatible_repo(
    source_roots: &[String],
    source_remote: &str,
    target_roots: &[String],
    target_remote: Option<&str>,
) -> bool {
    target_remote == Some(source_remote)
        && source_roots.iter().any(|root| target_roots.contains(root))
}

#[must_use]
pub fn conversations(events: &[Event], current: Option<&str>) -> Vec<String> {
    let mut result: Vec<String> =
        current.map(String::from).into_iter().collect();
    for event in events {
        let anchor = match &event.kind {
            EventKind::CheckpointCreated { checkpoint } => {
                Some(&checkpoint.history)
            }
            EventKind::UndoHistoryRestored { history, .. }
            | EventKind::HistoryMaterialized { history }
            | EventKind::NativeTurnStarted { history, .. } => Some(history),
            _ => None,
        };
        if let Some(conversation) = anchor.and_then(materialized)
            && !result.iter().any(|known| known == conversation)
        {
            result.push(String::from(conversation));
        }
    }
    result
}

fn materialized(anchor: &proto::HistoryAnchor) -> Option<&str> {
    use proto::HistoryAnchor;
    match anchor {
        HistoryAnchor::Messages {
            conversation,
            message: Some(_),
        }
        | HistoryAnchor::Turns {
            conversation,
            turn: Some(_),
        }
        | HistoryAnchor::Fork {
            source: conversation,
            ..
        } => Some(conversation),
        _ => None,
    }
}

/// The bundle must contain each recorded checkpoint at its recorded commit.
///
/// # Errors
/// The session lacks its opening metadata or a recorded checkpoint.
pub fn validate_history(
    session: SessionId,
    source: &InstanceId,
    refs: &Refs,
    events: &[Event],
) -> Result<(), &'static str> {
    let mut created = 0;
    let mut workspace = 0;
    let mut tip = None;
    for event in events {
        if event.session != session {
            return Err("the transfer includes another session's events");
        }
        match &event.kind {
            EventKind::SessionCreated {
                instance, provider, ..
            } if instance == source && *provider == source.provider => {
                created += 1;
            }
            EventKind::SessionCreated { .. } => {
                return Err(
                    "the transferred session has a different source account",
                );
            }
            EventKind::WorkspaceCreated { .. } => workspace += 1,
            EventKind::CheckpointCreated { checkpoint } => {
                if refs.checkpoints.get(&checkpoint.id.0)
                    != Some(&checkpoint.commit)
                {
                    return Err(
                        "the Git bundle is missing a recorded checkpoint",
                    );
                }
                tip = Some(&checkpoint.commit);
            }
            EventKind::SessionRemoved { .. } => {
                return Err("a removed session cannot move");
            }
            _ => {}
        }
    }
    if created == 1 && workspace == 1 && tip == Some(&refs.tip) {
        Ok(())
    } else {
        Err("the transferred history is missing the session's workspace")
    }
}

/// The new working directory still needs to reach the provider.
#[must_use]
pub fn notice_pending(history: &[Event]) -> bool {
    history
        .iter()
        .rev()
        .find_map(|event| match event.kind {
            EventKind::SessionArrivalReady { .. } => Some(true),
            EventKind::SessionMoveNoticeSent
            | EventKind::SessionMoved { .. } => Some(false),
            _ => None,
        })
        .unwrap_or(false)
}

#[must_use]
pub fn resume_for_message(history: &[Event]) -> bool {
    notice_pending(history)
        && !history
            .iter()
            .rev()
            .find_map(|event| crate::restore::agent_activity(&event.kind))
            .unwrap_or(false)
}

/// The parts of a settled source session identified by its event log.
pub struct SessionFiles {
    pub instance: InstanceId,
    pub workspace: Workspace,
    pub checkpoint: String,
    pub conversations: Vec<String>,
}

impl SessionFiles {
    /// # Errors
    /// Source history lacks the provider, workspace, or final checkpoint.
    pub fn of(events: &[Event]) -> Result<Self, &'static str> {
        let instance = events
            .iter()
            .find_map(|event| match &event.kind {
                EventKind::SessionCreated { instance, .. } => {
                    Some(instance.clone())
                }
                _ => None,
            })
            .ok_or("the source session has no provider")?;
        let workspace = events
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                EventKind::WorkspaceCreated { workspace } => {
                    Some(workspace.clone())
                }
                _ => None,
            })
            .ok_or("the source session has no workspace")?;
        let checkpoint = events
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                EventKind::CheckpointCreated { checkpoint } => {
                    Some(checkpoint.commit.clone())
                }
                _ => None,
            })
            .ok_or("the source session has no move checkpoint")?;
        let conversations = conversations(events, None);
        Ok(Self {
            instance,
            workspace,
            checkpoint,
            conversations,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settle {
    Snapshot,
    AlreadySettled,
    OtherMove,
}

impl Ownership {
    #[must_use]
    pub fn settle(self, target: NodeId, move_id: MoveId) -> Settle {
        match self {
            Self::Local => Settle::Snapshot,
            Self::Moving {
                to,
                id,
                phase: Phase::Settling,
            } if (to, id) == (target, move_id) => Settle::Snapshot,
            Self::Moving { to, id, .. } | Self::Released { to, id }
                if (to, id) == (target, move_id) =>
            {
                Settle::AlreadySettled
            }
            _ => Settle::OtherMove,
        }
    }
}

#[must_use]
pub const fn provider_pin(pin: agents::pin::Cli) -> &'static str {
    match pin {
        agents::pin::Cli::Claude => agents::pin::pins::CLAUDE_VERSION,
        agents::pin::Cli::Codex => agents::pin::pins::CODEX_VERSION,
    }
}

/// A verified source ref and its destination during workspace setup.
pub struct RefImport<'a> {
    pub refspec: String,
    pub destination: String,
    pub commit: &'a str,
}

impl Refs {
    #[must_use]
    pub fn imports(&self, session: SessionId) -> Vec<RefImport<'_>> {
        let head = head(session);
        let staging = incoming(session);
        self.mappings(session)
            .into_iter()
            .map(|(name, commit)| {
                let destination = if name == head {
                    staging.clone()
                } else {
                    name.clone()
                };
                RefImport {
                    refspec: format!("{name}:{destination}"),
                    destination,
                    commit,
                }
            })
            .collect()
    }
}

/// # Errors
/// The existing branch was not installed by the last local owner.
pub fn replaceable_branch(
    found: &str,
    retained: Option<&str>,
) -> Result<(), GitError> {
    if found.is_empty() || Some(found) == retained {
        Ok(())
    } else {
        Err(GitError::failed(
            "session move",
            "the target session branch was changed outside Conductor",
        ))
    }
}

#[must_use]
pub fn arrival_id(history: &[Event]) -> Option<MoveId> {
    history.iter().rev().find_map(|event| match event.kind {
        EventKind::SessionArrivalReady { move_id } => Some(move_id),
        _ => None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reception {
    Vacant,
    Returning,
    Owned,
}

impl Reception {
    #[must_use]
    pub fn after(history: &[Event]) -> Self {
        if history.is_empty() {
            Self::Vacant
        } else if matches!(
            Ownership::replay(history),
            Ownership::Released { .. }
        ) {
            Self::Returning
        } else {
            Self::Owned
        }
    }
}

#[must_use]
pub fn portable_name(name: &str) -> bool {
    !name.is_empty()
        && !matches!(name, "." | "..")
        && !name.contains(['/', '\\', ':'])
        && !name.chars().any(char::is_control)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectFormat {
    Sha1,
    Sha256,
}

impl ObjectFormat {
    /// # Errors
    /// The source repository uses a format this host cannot import.
    pub fn read(format: &str) -> Result<Self, &'static str> {
        match format {
            "sha1" => Ok(Self::Sha1),
            "sha256" => Ok(Self::Sha256),
            _ => Err("unknown Git object format"),
        }
    }

    #[must_use]
    pub const fn git(self) -> &'static str {
        match self {
            Self::Sha1 => "sha1",
            Self::Sha256 => "sha256",
        }
    }
}

/// Confirms the provider actually ran the selected transcript pin.
///
/// # Errors
/// The probe failed or reported a different version.
pub fn verified_pin(
    pin: &str,
    code: Option<i32>,
    stdout: &str,
) -> Result<String, String> {
    if code == Some(0)
        && stdout.split_ascii_whitespace().any(|word| word == pin)
    {
        Ok(String::from(pin))
    } else {
        Err(format!("session moves require CLI pin {pin} on both hosts"))
    }
}

#[must_use]
pub fn compatible_local(
    source_roots: &[String],
    retained_roots: &[String],
    observed_origin: Option<&str>,
) -> bool {
    observed_origin.is_none()
        && source_roots
            .iter()
            .any(|root| retained_roots.contains(root))
}

#[cfg(test)]
mod tests {
    use super::{Arrival, Incoming, MoveId, Ownership, Phase, Settle, head};
    use proto::{
        Checkpoint, CheckpointId, ClientName, Epoch, Event, EventKind,
        HistoryAnchor, HostPath, HostSeq, InstanceId, Provider, SessionId,
        UndoAvailability, Workspace,
    };

    fn path(value: &str) -> HostPath {
        HostPath(String::from(value))
    }

    #[test]
    fn a_retry_settles_only_the_same_move() {
        let active = Ownership::Moving {
            to: [9; 32],
            id: MoveId(1),
            phase: Phase::SendingHistory,
        };
        assert_eq!(active.settle([9; 32], MoveId(1)), Settle::AlreadySettled);
        assert_eq!(active.settle([8; 32], MoveId(1)), Settle::OtherMove);
        assert_eq!(active.settle([9; 32], MoveId(2)), Settle::OtherMove);
    }

    #[test]
    fn an_arrival_rewrites_paths_and_checkpoint_cursor_references()
    -> Result<(), crate::workspace::GitError> {
        let id = SessionId(7);
        let target_account = InstanceId::own(Provider::Claude);
        let target = Workspace {
            original: path("/target/project"),
            repository: path("/target/project"),
            directory: path("/target/worktree"),
            worktree: path("/target/worktree"),
            branch: head(id),
        };
        let event = |seq, kind| Event {
            seq: HostSeq(seq),
            session: id,
            kind,
        };
        let history = Vec::from([
            event(
                2,
                EventKind::SessionCreated {
                    instance: target_account.clone(),
                    provider: Provider::Claude,
                    directory: path("/source/project"),
                    by: ClientName(String::from("Tests")),
                    model: None,
                    effort: None,
                },
            ),
            event(
                4,
                EventKind::UserMessage {
                    text: String::from("synthetic turn"),
                    by: ClientName(String::from("Tests")),
                },
            ),
            event(
                9,
                EventKind::CheckpointCreated {
                    checkpoint: Checkpoint {
                        id: CheckpointId(1),
                        parent: Some(CheckpointId(0)),
                        turn_start: Some(HostSeq(4)),
                        commit: String::from("synthetic-commit"),
                        history: HistoryAnchor::Fresh,
                        undo: UndoAvailability::Available,
                    },
                },
            ),
        ]);
        let arrival = Arrival {
            incoming: Incoming {
                from: [1; 32],
                id: MoveId(3),
                session: id,
            },
            source_epoch: Epoch(1),
            source_last_seq: HostSeq(9),
            destination: target_account,
            workspace: target.clone(),
            history,
            conversation: None,
        };
        let events = arrival.events(HostSeq(20))?;
        assert!(arrival.incoming.receipt(&events));
        let original = events.iter().find_map(|event| match &event.kind {
            EventKind::SessionCreated { directory, .. } => Some(directory),
            _ => None,
        });
        assert_eq!(original, Some(&target.original));
        let user = events
            .iter()
            .find(|event| matches!(event.kind, EventKind::UserMessage { .. }))
            .map(|event| event.seq);
        let start = events.iter().find_map(|event| match &event.kind {
            EventKind::CheckpointCreated { checkpoint } => {
                checkpoint.turn_start
            }
            _ => None,
        });
        assert_eq!(start, user);
        assert_ne!(start, Some(HostSeq(4)));
        assert!(matches!(
            events.last().map(|event| &event.kind),
            Some(EventKind::SessionArrivalReady { move_id: MoveId(3) })
        ));
        Ok(())
    }
}
