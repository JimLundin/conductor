use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use agents::{ClientName, TurnOutcome};
use anyhow::Context;
use host::session::{
    EventKind, Order, SessionDirectory, SessionId, SessionRefusal,
    SessionResult,
};
use host::workspace::{Snapshot, SnapshotDiff, Workspace};
use project_runtime::{Completion, Failure, VersionControl};
use vcs::command::{
    Capture, Compare, Comparison, Install, Integrate, Prepare, Prepared,
    PreserveAndRemove, Revision, WorkspaceId,
};

use super::Fixture;

fn started(fixture: &Fixture) -> EventKind {
    EventKind::SessionCreated {
        instance: fixture.source.clone(),
        directory: SessionDirectory(String::from("/work")),
        model: None,
        effort: None,
        by: ClientName(String::from("owner")),
    }
}

#[tokio::test]
async fn restart_reads_the_log_once_and_interrupts_only_the_session_in_a_turn()
-> anyhow::Result<()> {
    let mut fixture = Fixture::new()?;
    let log = fixture.actor.log.clone();
    let (observed, reads) = log.observe_history_reads();
    fixture.actor.log = observed;
    let busy = SessionId(4);
    let idle = SessionId(2);
    log.append(busy, started(&fixture)).await?;
    log.append(idle, started(&fixture)).await?;
    log.append(busy, EventKind::SessionReady).await?;
    log.append(idle, EventKind::SessionReady).await?;
    log.append(idle, EventKind::NativeTurnStarted { turn_start: None })
        .await?;
    log.append(
        idle,
        EventKind::TurnCompleted {
            outcome: TurnOutcome::Completed,
        },
    )
    .await?;
    let start = log
        .record(busy, EventKind::NativeTurnStarted { turn_start: None })
        .await?;

    fixture.actor.restore().await?;
    assert_eq!(*reads.borrow(), 1);
    let recovered = log.subscribe(start.seq).await?.replay;
    let closed = EventKind::SessionClosed {
        reason: host::restore::restarted(),
    };
    assert_eq!(
        recovered
            .iter()
            .map(|event| (event.session, &event.kind))
            .collect::<Vec<_>>(),
        [
            (
                busy,
                &EventKind::TurnCompleted {
                    outcome: TurnOutcome::Interrupted
                }
            ),
            (busy, &closed),
            (idle, &closed),
        ]
    );
    let last = recovered.last().context("missing recovery events")?.seq;
    fixture.actor.restore().await?;
    assert_eq!(*reads.borrow(), 2);
    assert_eq!(log.subscribe(last).await?.replay, []);
    Ok(())
}

struct Snapshots {
    captures: Sender<WorkspaceId>,
    fails: WorkspaceId,
}

impl VersionControl for Snapshots {
    fn prepare(&self, _: Prepare) -> Completion<'_, Prepared> {
        Box::pin(async { Err(Failure::Unavailable) })
    }

    fn capture(&self, request: Capture) -> Completion<'_, Revision> {
        let id = request.workspace.id;
        let _ = self.captures.send(id.clone());
        Box::pin(async move {
            if id == self.fails {
                Err(Failure::Workspace(vcs::Error::Failed(String::from(
                    "capture failed",
                ))))
            } else {
                Ok(Revision(String::from("captured")))
            }
        })
    }

    fn install(&self, _: Install) -> Completion<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    fn compare(&self, request: Compare) -> Completion<'_, Comparison> {
        Box::pin(async move {
            if request.baseline.0 != "base" || request.revision.0 != "captured"
            {
                return Err(Failure::Unavailable);
            }
            Ok(Comparison {
                files: Vec::new(),
                extent: vcs::DiffExtent::Complete,
            })
        })
    }

    fn integrate(&self, _: Integrate) -> Completion<'_, u64> {
        Box::pin(async { Err(Failure::Unavailable) })
    }

    fn preserve_and_remove(
        &self,
        _: PreserveAndRemove,
    ) -> Completion<'_, vcs::Kept> {
        Box::pin(async { Err(Failure::Unavailable) })
    }
}

fn use_snapshots(
    fixture: &mut Fixture,
    fail: SessionId,
) -> Receiver<WorkspaceId> {
    let (captures, attempts) = mpsc::channel();
    fixture.actor.workspaces = crate::Workspaces::new(
        Arc::new(Snapshots {
            captures,
            fails: WorkspaceId(fail.to_string()),
        }),
        vcs_git2::registration::register,
    );
    attempts
}

async fn start_turn(
    fixture: &Fixture,
    session: SessionId,
    workspace: bool,
) -> anyhow::Result<()> {
    let log = &fixture.actor.log;
    log.append(session, started(fixture)).await?;
    if workspace {
        let workspace = Workspace {
            repository: vcs::Repository(String::from("/repo")),
            original: vcs::WorkingCopy(String::from("/repo")),
            worktree: vcs::Worktree(format!("/work/{session}")),
            directory: SessionDirectory(format!("/work/{session}")),
            branch: session.to_string(),
        };
        log.append(session, EventKind::WorkspaceCreated { workspace })
            .await?;
        let snapshot = Snapshot {
            turn_start: None,
            commit: String::from("base"),
        };
        log.append(session, EventKind::SnapshotRecorded { snapshot })
            .await?;
    }
    log.append(session, EventKind::NativeTurnStarted { turn_start: None })
        .await?;
    Ok(())
}

#[tokio::test]
async fn restored_sessions_use_the_recorded_snapshot_outcome_and_sequence()
-> anyhow::Result<()> {
    let mut fixture = Fixture::new()?;
    let kept = SessionId(9);
    let failed = SessionId(3);
    let without = SessionId(5);
    let attempts = use_snapshots(&mut fixture, failed);
    start_turn(&fixture, kept, true).await?;
    start_turn(&fixture, failed, true).await?;
    start_turn(&fixture, without, false).await?;
    fixture.actor.restore().await?;
    assert_eq!(
        attempts.try_iter().map(|id| id.0).collect::<Vec<_>>(),
        [kept.to_string(), failed.to_string()]
    );

    let history = fixture.actor.log.history().await?;
    let recorded = |id, matches: fn(&EventKind) -> bool| {
        history
            .iter()
            .find(|event| event.session == id && matches(&event.kind))
            .map(|event| event.seq)
    };
    let captured = recorded(kept, |kind| {
        matches!(kind, EventKind::SnapshotRecorded { snapshot } if snapshot.commit == "captured")
    })
    .context("missing successful snapshot")?;
    let error = recorded(failed, |kind| {
        matches!(kind, EventKind::Error { message } if message.contains("capture failed"))
    })
    .context("missing snapshot failure")?;
    let restored = |id| {
        fixture
            .actor
            .core
            .find(id)
            .with_context(|| format!("session {id} was not restored"))
    };
    assert_eq!(
        restored(kept)?.order(Order::Diff(captured)).await?,
        SessionResult::Diff(SnapshotDiff {
            snapshot: captured,
            files: Vec::new(),
            extent: vcs::DiffExtent::Complete
        })
    );
    assert_eq!(
        restored(failed)?.order(Order::Diff(error)).await?,
        SessionResult::Refused(SessionRefusal::SnapshotUnavailable)
    );
    Ok(())
}
