use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use anyhow::{Context, ensure};
use clap::Parser;
use iroh::endpoint::{Connection, SendStream, presets};
use iroh::{Endpoint, EndpointAddr, RelayMode};
use net::{LinkOptions, Network, Target, wire};
use proto::{
    Answer, AuthToken, Catalog, ClientName, CommandBody, CommandResult, Epoch,
    Event, EventKind, Feed, HelloReply, HostPath, HostSeq, InstanceId,
    Instances, Open, Provider, Question, Rejection, RequestId, Selection,
    SessionId, Ticket, Welcome,
};

use super::{Args, run_to_deadline};

const CHILD: &str = "fixture::headless_process_filters_concurrent_transcripts_and_fails_on_rejection";
const TICKET_ENV: &str = "CONDUCTOR_HEADLESS_FIXTURE_TICKET";
const ADDR_ENV: &str = "CONDUCTOR_HEADLESS_FIXTURE_ADDRESS";
const SELECTED: SessionId = SessionId(1);
const OTHER: SessionId = SessionId(2);
const REQUEST: RequestId = RequestId(1);
const MARKER: &str = "outside-transcript-marker";
const SELECTED_REPLY: &str = "selected-transcript-marker";
const TOKEN: AuthToken = AuthToken::from_bytes([7; 16]);

#[tokio::test]
async fn headless_process_filters_concurrent_transcripts_and_fails_on_rejection()
-> anyhow::Result<()> {
    if let Ok(ticket) = std::env::var(TICKET_ENV) {
        return fixture_client(&ticket).await;
    }
    let endpoint = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .alpns(vec![proto::ALPN.to_vec()])
        .bind()
        .await?;
    let port = endpoint
        .bound_sockets()
        .into_iter()
        .find(SocketAddr::is_ipv4)
        .context("fixture has no IPv4 socket")?
        .port();
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let ticket = Ticket {
        host: *endpoint.id().as_bytes(),
        token: TOKEN,
        relay: String::from("https://fixture.invalid/"),
    };
    let mut child = tokio::process::Command::new(std::env::current_exe()?);
    child
        .args(["--exact", CHILD, "--nocapture"])
        .env(TICKET_ENV, ticket.to_text())
        .env(ADDR_ENV, addr.to_string())
        .kill_on_drop(true);
    let (child_result, served) =
        tokio::time::timeout(Duration::from_secs(15), async {
            tokio::join!(child.output(), serve(&endpoint))
        })
        .await?;
    served?;
    let output = child_result?;
    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;
    ensure!(!output.status.success(), "rejection succeeded: {stdout}");
    ensure!(
        stdout.contains(SELECTED_REPLY),
        "missing own transcript: {stdout}"
    );
    ensure!(
        !stdout.contains(MARKER) && !stderr.contains(MARKER),
        "leaked another session"
    );
    ensure!(
        stderr.contains("command was not accepted: Rejected(UnknownRequest)"),
        "wrong failure: {stderr}"
    );
    endpoint.close().await;
    Ok(())
}

async fn fixture_client(ticket: &str) -> anyhow::Result<()> {
    let addr = std::env::var(ADDR_ENV)?.parse::<SocketAddr>()?;
    let args = Args::try_parse_from([
        "headless",
        ticket,
        "--directory",
        "/fixture",
        "--prompt",
        "selected question",
        "--timeout-secs",
        "10",
    ])?;
    let address =
        EndpointAddr::new(iroh::EndpointId::from_bytes(&args.ticket.host)?)
            .with_ip_addr(addr);
    let target = Target {
        addr: address,
        token: args.ticket.token,
    };
    let options = LinkOptions {
        network: Network::Local,
        ..LinkOptions::new(ClientName(String::from("headless")))
    };
    run_to_deadline(args, target, options).await
}

async fn serve(endpoint: &Endpoint) -> anyhow::Result<()> {
    let conn = endpoint
        .accept()
        .await
        .context("no fixture connection")?
        .await?;
    hello(&conn).await?;
    let mut feed = subscribe(&conn).await?;
    let start = command(&conn).await?;
    ensure!(matches!(start.1, CommandBody::StartSession { .. }));
    respond(start.0, CommandResult::Started { session: SELECTED }).await?;
    let message = command(&conn).await?;
    ensure!(matches!(
        message.1,
        CommandBody::SendMessage {
            session: SELECTED,
            ..
        }
    ));
    respond(message.0, CommandResult::Accepted).await?;
    send_concurrent_events(&mut feed).await?;
    reject_answer(&conn).await?;
    let _ = conn.closed().await;
    Ok(())
}

async fn hello(conn: &Connection) -> anyhow::Result<()> {
    let (mut send, mut recv) = conn.accept_bi().await?;
    let Some(Open::Hello(hello)) = wire::read(&mut recv).await? else {
        anyhow::bail!("expected fixture hello");
    };
    hello.token.check(&TOKEN)?;
    wire::write(
        &mut send,
        &HelloReply::Welcome(Welcome {
            host: String::from("fixture"),
            epoch: Epoch(1),
            head_seq: HostSeq(0),
            instances: Instances::default(),
            catalog: Catalog::default(),
            home: None,
        }),
    )
    .await?;
    send.finish()?;
    Ok(())
}

async fn subscribe(conn: &Connection) -> anyhow::Result<SendStream> {
    let (mut send, mut recv) = conn.accept_bi().await?;
    let frame = wire::read(&mut recv).await?;
    ensure!(matches!(frame, Some(Open::Subscribe(_))));
    wire::write(&mut send, &Feed::Instances(Instances::default())).await?;
    Ok(send)
}

async fn command(
    conn: &Connection,
) -> anyhow::Result<(SendStream, CommandBody)> {
    let (send, mut recv) = conn.accept_bi().await?;
    match wire::read(&mut recv).await? {
        Some(Open::Command(command)) => Ok((send, command.body)),
        _ => anyhow::bail!("expected fixture command"),
    }
}

async fn respond(
    mut send: SendStream,
    result: CommandResult,
) -> anyhow::Result<()> {
    wire::write(&mut send, &result).await?;
    send.finish()?;
    Ok(())
}

fn created() -> EventKind {
    EventKind::SessionCreated {
        instance: InstanceId::own(Provider::Claude),
        provider: Provider::Claude,
        directory: HostPath(String::from("/fixture")),
        by: ClientName(String::from("fixture")),
        model: None,
        effort: None,
    }
}

fn message(text: &str) -> EventKind {
    EventKind::UserMessage {
        text: String::from(text),
        by: ClientName(String::from("fixture")),
    }
}

async fn send_concurrent_events(send: &mut SendStream) -> anyhow::Result<()> {
    let events = [
        (OTHER, created()),
        (SELECTED, created()),
        (OTHER, EventKind::SessionReady),
        (SELECTED, EventKind::SessionReady),
        (OTHER, message(MARKER)),
        (SELECTED, message("selected question")),
        (
            OTHER,
            EventKind::AssistantMessage {
                text: String::from(MARKER),
            },
        ),
        (
            SELECTED,
            EventKind::AssistantMessage {
                text: String::from(SELECTED_REPLY),
            },
        ),
        (
            SELECTED,
            EventKind::QuestionAsked {
                request: REQUEST,
                questions: vec![Question {
                    header: String::from("fixture"),
                    text: String::from("Continue?"),
                    options: Vec::new(),
                    selection: Selection::One,
                }],
            },
        ),
    ];
    let mut seq = HostSeq(0);
    for (session, kind) in events {
        seq = seq.next();
        wire::write(send, &Feed::Event(Event { session, kind, seq })).await?;
    }
    Ok(())
}

async fn reject_answer(conn: &Connection) -> anyhow::Result<()> {
    let (reply, request) = command(conn).await?;
    ensure!(
        request
            == CommandBody::Answer {
                session: SELECTED,
                request: REQUEST,
                answers: vec![Answer(vec![String::from("yes")])],
            }
    );
    respond(reply, CommandResult::Rejected(Rejection::UnknownRequest)).await
}
