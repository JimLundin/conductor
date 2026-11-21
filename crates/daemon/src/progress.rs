//! Notes the progress of sessions on issues as their turns end (docs/home.md
//! §4). Which turn ends note what is `host::progress`'s decision; this task
//! follows the log and writes each note through the home.

use std::convert::Infallible;

use host::progress::Progress;
use host::serve::HomeOrder;
use proto::{Event, HostSeq, IssueChange};
use tokio::sync::broadcast;

use crate::Stopped;
use crate::home::HomeClone;
use crate::log::Log;

pub fn spawn(log: Log, home: HomeClone) {
    tokio::spawn(async move {
        let stopped = watch(&log, &home).await;
        tracing::warn!(?stopped, "progress notes stopped");
    });
}

async fn watch(log: &Log, home: &HomeClone) -> Result<Infallible, Stopped> {
    let mut seen = host::progress::latest(&log.history().await?);
    loop {
        let watched = log.subscribe(seen).await?;
        for event in watched.replay {
            seen = note(log, home, &event).await?;
        }
        seen = follow(log, home, watched.live, seen).await?;
    }
}

/// Notes each live event's progress, until the subscription lags behind;
/// then the last event it saw.
async fn follow(
    log: &Log,
    home: &HomeClone,
    mut live: broadcast::Receiver<Event>,
    mut seen: HostSeq,
) -> Result<HostSeq, Stopped> {
    loop {
        match live.recv().await {
            Ok(event) => seen = note(log, home, &event).await?,
            Err(broadcast::error::RecvError::Lagged(_)) => return Ok(seen),
            Err(broadcast::error::RecvError::Closed) => return Err(Stopped {}),
        }
    }
}

/// Notes what the turn `event` ends did on its issue, if it ends one.
async fn note(
    log: &Log,
    home: &HomeClone,
    event: &Event,
) -> Result<HostSeq, Stopped> {
    if host::progress::ends_turn(event) {
        let history = log.history().await?;
        if let Some(progress) = host::progress::progress(&history, event) {
            write(home, progress).await?;
        }
    }
    Ok(event.seq)
}

async fn write(home: &HomeClone, progress: Progress) -> Result<(), Stopped> {
    let Progress { assignment, text } = progress;
    let signer = assignment.signer();
    let issue = assignment.issue;
    let answer = home
        .order(
            HomeOrder::Change(issue, IssueChange::Note(text), signer),
            proto::CommandId(ulid::Ulid::new().0),
        )
        .await?;
    tracing::info!(target: "home", ?issue, ?answer, "noted a turn's progress");
    Ok(())
}
