//! Seeds an existing fake host for the phone diff/undo browser scenario.
//! The supplied directory is a committed Git repo holding claude-worktrees.jsonl
//! as stdout.jsonl; the daemon remains live after this helper exits.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use client::{HostLink, LinkOptions, LinkState, Status, Target, Update};
use proto::{
    ClientName, CommandBody, CommandResult, EventKind, HostPath, Provider, SessionId, Ticket,
};
use tokio::sync::mpsc;

#[derive(Parser)]
struct Args {
    #[arg(value_parser = Ticket::parse)]
    ticket: Ticket,
    directory: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let (link, mut updates) = HostLink::spawn(
        Target::of_ticket(&args.ticket),
        LinkOptions::new(ClientName(String::from("browser seed"))),
    )
    .await?;
    drive_live(&mut updates).await?;
    let session = match link
        .send(CommandBody::StartSession {
            provider: Provider::Claude,
            directory: HostPath(args.directory.display().to_string()),
            model: None,
            effort: None,
        })
        .await?
    {
        CommandResult::Started { session } => Some(session),
        _ => None,
    }
    .context("the fake Git session starts")?;
    turn(&link, &mut updates, session, "turn one").await?;
    turn(&link, &mut updates, session, "turn two").await?;
    turn(&link, &mut updates, session, "turn three").await?;
    link.stop().await?;
    Ok(())
}

async fn next(updates: &mut mpsc::Receiver<Update>) -> anyhow::Result<Update> {
    tokio::time::timeout(Duration::from_secs(30), updates.recv())
        .await
        .context("a fake-host update arrives")?
        .context("the fake-host link stays open")
}

async fn drive_live(updates: &mut mpsc::Receiver<Update>) -> anyhow::Result<()> {
    loop {
        match next(updates).await? {
            Update::Status(Status {
                state: LinkState::Live,
                ..
            }) => break Ok(()),
            Update::Status(_) | Update::Reset(_) | Update::Event(_) => {}
        }
    }
}

async fn turn(
    link: &HostLink,
    updates: &mut mpsc::Receiver<Update>,
    session: SessionId,
    text: &str,
) -> anyhow::Result<()> {
    match link
        .send(CommandBody::SendMessage {
            session,
            text: String::from(text),
        })
        .await?
    {
        CommandResult::Accepted => Some(()),
        _ => None,
    }
    .context("the fake host accepts its next turn")?;
    drive_turn(updates, session).await
}

async fn drive_turn(
    updates: &mut mpsc::Receiver<Update>,
    session: SessionId,
) -> anyhow::Result<()> {
    loop {
        match next(updates).await? {
            Update::Event(event) if event.session == session => {
                if let EventKind::TurnCompleted { .. } = event.kind {
                    break Ok(());
                }
            }
            Update::Status(_) | Update::Reset(_) | Update::Event(_) => {}
        }
    }
}
