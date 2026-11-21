//! Notes the progress of sessions on issues as their turns end (docs/home.md
//! §4). Which turn ends note what is `host::progress`'s decision; this task
//! follows the log and writes each note through the home.

use std::convert::Infallible;
use std::time::Duration;

use host::log::Completion;

use host::progress::Progress;
use host::serve::HomeOrder;
use proto::{CommandResult, Epoch, Event, HostSeq, IssueChange};
use tokio::sync::broadcast;

use crate::Stopped;
use crate::home::HomeClone;
use crate::log::Log;

const RETRY_DELAY: Duration = Duration::from_secs(2);

pub fn spawn(log: Log, home: HomeClone) {
    tokio::spawn(async move {
        let stopped = watch(&log, &home).await;
        tracing::warn!(?stopped, "progress notes stopped");
    });
}

async fn watch(log: &Log, home: &HomeClone) -> Result<Infallible, Stopped> {
    let (epoch, mut seen) = log.progress_cursor().await?;
    loop {
        let watched = log.subscribe(seen).await?;
        for event in watched.replay {
            seen = note(log, home, epoch, &event).await?;
        }
        seen = follow(log, home, epoch, watched.live, seen).await?;
    }
}

/// Notes each live event's progress, until the subscription lags behind;
/// then the last event it saw.
async fn follow(
    log: &Log,
    home: &HomeClone,
    epoch: Epoch,
    mut live: broadcast::Receiver<Event>,
    mut seen: HostSeq,
) -> Result<HostSeq, Stopped> {
    loop {
        match live.recv().await {
            Ok(event) => seen = note(log, home, epoch, &event).await?,
            Err(broadcast::error::RecvError::Lagged(_)) => return Ok(seen),
            Err(broadcast::error::RecvError::Closed) => return Err(Stopped {}),
        }
    }
}

/// Notes what the turn `event` ends did on its issue, if it ends one.
async fn note(
    log: &Log,
    home: &HomeClone,
    epoch: Epoch,
    event: &Event,
) -> Result<HostSeq, Stopped> {
    if host::progress::ends_turn(event) {
        let history = log.history().await?;
        if let Some(progress) = host::progress::progress(&history, event) {
            write(log, home, progress, epoch, event.seq).await?;
        }
    }
    Ok(event.seq)
}

async fn write(
    log: &Log,
    home: &HomeClone,
    progress: Progress,
    epoch: Epoch,
    seq: HostSeq,
) -> Result<(), Stopped> {
    let Progress { assignment, text } = progress;
    let signer = assignment.signer();
    let issue = assignment.issue;
    let id = host::progress::edit_id(epoch, seq);
    let order = HomeOrder::Change(issue, IssueChange::Note(text), signer);
    loop {
        match home.order(order.clone(), id).await? {
            Completion::Final(CommandResult::Accepted) => {
                log.progress_delivered(seq).await?;
                tracing::info!(target: "home", ?issue, progress_seq = seq.0, "noted a turn's progress");
                return Ok(());
            }
            Completion::Final(_) => {
                tracing::warn!(target: "home", ?issue, progress_seq = seq.0, "progress note rejected");
                return Ok(());
            }
            Completion::Retryable(_) => {
                tracing::warn!(target: "home", ?issue, progress_seq = seq.0, "progress note will retry");
                tokio::select! {
                    () = log.stopped() => return Err(Stopped {}),
                    () = tokio::time::sleep(RETRY_DELAY) => {}
                }
            }
        }
    }
}
