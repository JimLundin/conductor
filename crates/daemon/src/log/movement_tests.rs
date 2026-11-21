use host::movement::{Arrival, Incoming};
use proto::movement::MoveId;
use proto::{
    ClientName, Epoch, Event, EventKind, HostPath, HostSeq, InstanceId,
    Provider, RequestId, Resolution, SessionId, Workspace,
};
use std::path::{Path, PathBuf};

use super::Log;

#[tokio::test]
async fn two_host_logs_commit_the_arrival_once_and_accept_a_return()
-> anyhow::Result<()> {
    let scratch = Scratch::new()?;
    let source_dir = scratch.0.join("source");
    let target_dir = scratch.0.join("target");
    std::fs::create_dir(&source_dir)?;
    std::fs::create_dir(&target_dir)?;
    let source =
        Log::open(String::from("Source"), &source_dir.join("host.redb"))?;
    let target_path = target_dir.join("host.redb");
    let target = Log::open(String::from("Target"), &target_path)?;
    let id = SessionId(121);
    seed(&source, id, workspace(&source_dir)).await?;

    let first = Incoming {
        from: [1; 32],
        id: MoveId(1),
        session: id,
    };
    let local = source.finish_history(id).await?;
    target
        .arrive(arrival(first, local.clone(), workspace(&target_dir))?)
        .await
        .map_err(anyhow::Error::msg)?;
    target
        .arrive(arrival(first, local, workspace(&target_dir))?)
        .await
        .map_err(anyhow::Error::msg)?;
    target.shutdown().await?;
    target.stopped().await;

    let target = Log::open(String::from("Target"), &target_path)?;
    let imported = target.finish_history(id).await?;
    assert!(first.receipt(&imported));
    assert_visible_once(&imported);
    source
        .append(
            id,
            EventKind::SessionMoved {
                to: [2; 32],
                move_id: first.id,
            },
        )
        .await?;
    target.append(id, message("new turn on the target")).await?;
    let second = Incoming {
        from: [2; 32],
        id: MoveId(2),
        session: id,
    };
    let before = source.finish_history(id).await?;
    let mut bad = target.finish_history(id).await?;
    bad.retain(|event| !matches!(event.kind, EventKind::UserMessage { .. }));
    assert!(
        source
            .arrive(arrival(second, bad, workspace(&source_dir))?)
            .await
            .is_err()
    );
    assert_eq!(source.finish_history(id).await?, before);

    let returning = arrival(
        second,
        target.finish_history(id).await?,
        workspace(&source_dir),
    )?;
    source.arrive(returning).await.map_err(anyhow::Error::msg)?;
    target
        .append(
            id,
            EventKind::SessionMoved {
                to: [1; 32],
                move_id: second.id,
            },
        )
        .await?;
    let returned = source.finish_history(id).await?;
    assert!(second.receipt(&returned));
    assert_visible_once(&returned);
    assert_eq!(
        returned
            .iter()
            .filter_map(|event| match &event.kind {
                EventKind::UserMessage { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        ["first turn", "new turn on the target"]
    );
    source.shutdown().await?;
    target.shutdown().await?;
    source.stopped().await;
    target.stopped().await;
    Ok(())
}

#[tokio::test]
async fn moved_request_history_reaches_clients_without_generating_push()
-> anyhow::Result<()> {
    let source = Log::spawn(String::from("Source"), Epoch(1));
    let destination = Log::spawn(String::from("Destination"), Epoch(2));
    let id = SessionId(121);
    let request = RequestId(1);
    seed(&source, id, workspace(Path::new("source"))).await?;
    source
        .append(
            id,
            EventKind::QuestionAsked {
                request,
                questions: Vec::new(),
            },
        )
        .await?;
    source
        .append(
            id,
            EventKind::RequestResolved {
                request,
                resolution: Resolution::Withdrawn,
            },
        )
        .await?;

    let mut push = destination.follow_requests().await?;
    let mut client = destination.subscribe(HostSeq(0)).await?.live;
    let incoming = Incoming {
        from: [1; 32],
        id: MoveId(1),
        session: id,
    };
    let moved = arrival(
        incoming,
        source.finish_history(id).await?,
        workspace(Path::new("destination")),
    )?;
    destination
        .arrive(moved)
        .await
        .map_err(anyhow::Error::msg)?;

    let client_events =
        std::iter::from_fn(|| client.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        client_events
            .iter()
            .any(|event| event.kind.opened_request() == Some(request))
    );
    assert!(client_events.iter().any(|event| matches!(event.kind, EventKind::RequestResolved { request: reported, .. } if reported == request)));
    assert!(matches!(
        push.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    Ok(())
}

fn assert_visible_once(events: &[Event]) {
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.kind,
                EventKind::SessionArrivalReady { .. }
            ))
            .count(),
        1
    );
    assert_eq!(host::restore::created(events).len(), 1);
}

async fn seed(
    log: &Log,
    id: SessionId,
    workspace: Workspace,
) -> anyhow::Result<()> {
    log.append(
        id,
        EventKind::SessionCreated {
            instance: InstanceId::own(Provider::Claude),
            provider: Provider::Claude,
            directory: workspace.original.clone(),
            by: ClientName(String::from("Tests")),
            model: None,
            effort: None,
        },
    )
    .await?;
    log.append(id, EventKind::WorkspaceCreated { workspace })
        .await?;
    log.append(id, message("first turn")).await?;
    Ok(())
}

fn message(text: &str) -> EventKind {
    EventKind::UserMessage {
        text: String::from(text),
        by: ClientName(String::from("Tests")),
    }
}

fn arrival(
    incoming: Incoming,
    history: Vec<Event>,
    workspace: Workspace,
) -> anyhow::Result<Arrival> {
    let last = history
        .last()
        .ok_or_else(|| anyhow::anyhow!("expected source history"))?
        .seq;
    Ok(Arrival {
        incoming,
        source_epoch: Epoch(1),
        source_last_seq: last,
        destination: InstanceId::own(Provider::Claude),
        workspace,
        history,
        conversation: Some(String::from("synthetic-conversation")),
    })
}

fn workspace(data: &Path) -> Workspace {
    let path = |name| HostPath(data.join(name).display().to_string());
    Workspace {
        repository: path("repository"),
        original: path("repository"),
        directory: path("worktree"),
        worktree: path("worktree"),
        branch: String::from("test-session"),
    }
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> std::io::Result<Self> {
        let path = std::env::temp_dir()
            .join(format!("conductor-move-test-{}", ulid::Ulid::new()));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
