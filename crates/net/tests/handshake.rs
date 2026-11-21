#![cfg(test)]

use std::time::Duration;

use iroh::endpoint::{Connection, ConnectionError, SendStream, presets};
use iroh::{Endpoint, RelayMode};
use net::wire;
use net::{HostLink, LinkOptions, LinkState, Network, Status, Target, Update};
use proto::{
    AuthToken, Availability, ClientName, Epoch, Feed, HelloReply, HostSeq,
    InstanceId, InstanceStatus, Instances, Open, Provider, Welcome,
};
use tokio::sync::mpsc;

const STEP: Duration = Duration::from_secs(15);

type TestResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

async fn endpoint() -> TestResult<Endpoint> {
    Ok(Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .alpns(Vec::from([proto::ALPN.to_vec()]))
        .bind()
        .await?)
}

async fn link(
    endpoint: &Endpoint,
) -> TestResult<(HostLink, mpsc::Receiver<Update>)> {
    Ok(HostLink::spawn(
        Target {
            addr: endpoint.addr(),
            token: AuthToken::from_bytes([7; 16]),
        },
        LinkOptions {
            network: Network::Local,
            ..LinkOptions::new(ClientName(String::from("handshake-test")))
        },
    )
    .await?)
}

async fn accept_hello(
    endpoint: &Endpoint,
) -> TestResult<(Connection, SendStream)> {
    let conn = endpoint
        .accept()
        .await
        .ok_or("the endpoint closed before a connection")?
        .await?;
    let (send, mut recv) = conn.accept_bi().await?;
    let opening = wire::read::<Open>(&mut recv)
        .await?
        .ok_or("the stream ended before the opening")?;
    assert_eq!(
        Some(&opening).filter(|open| matches!(open, Open::Hello(_))),
        Some(&opening)
    );
    Ok((conn, send))
}

async fn stalled_hello(endpoint: &Endpoint) -> TestResult<()> {
    let (conn, _send) = accept_hello(endpoint).await?;
    let closed = tokio::time::timeout(STEP, conn.closed()).await?;
    assert_eq!(
        Some(&closed).filter(|reason| matches!(
            reason,
            ConnectionError::ApplicationClosed(_)
        )),
        Some(&closed)
    );
    Ok(())
}

async fn delayed_hello(endpoint: &Endpoint) -> TestResult<()> {
    let (conn, mut send) = accept_hello(endpoint).await?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    welcome(&mut send).await?;
    let (mut feed_send, _recv) = accept_subscription(&conn).await?;
    wire::write(&mut feed_send, &Feed::Instances(Instances::default())).await?;
    let _ = tokio::time::timeout(STEP, conn.closed()).await?;
    Ok(())
}

async fn welcome(send: &mut SendStream) -> TestResult<()> {
    let unavailable = InstanceStatus {
        availability: Availability::Unavailable {
            reason: String::from("handshake test"),
        },
    };
    wire::write(
        send,
        &HelloReply::Welcome(Welcome {
            host: String::from("test-host"),
            epoch: Epoch(11),
            head_seq: HostSeq(0),
            instances: Instances(
                [(InstanceId::own(Provider::Claude), unavailable)].into(),
            ),
            catalog: proto::Catalog::default(),
            home: None,
        }),
    )
    .await?;
    send.finish()?;
    Ok(())
}

async fn accept_subscription(
    conn: &Connection,
) -> TestResult<(SendStream, iroh::endpoint::RecvStream)> {
    let (send, mut recv) = conn.accept_bi().await?;
    let frame = wire::read::<Open>(&mut recv).await?;
    assert!(matches!(frame, Some(Open::Subscribe(_))));
    Ok((send, recv))
}

async fn until_live(
    updates: &mut mpsc::Receiver<Update>,
) -> TestResult<Vec<Status>> {
    let mut seen = Vec::new();
    loop {
        if let Update::Status(status) =
            updates.recv().await.ok_or("the link's updates ended")?
        {
            let live = status.state == LinkState::Live;
            seen.push(status);
            if live {
                break Ok(seen);
            }
        }
    }
}

#[tokio::test]
async fn stalled_hello_closes_the_connection_and_retries() -> TestResult<()> {
    let host = endpoint().await?;
    let server = host.clone();
    let serving = tokio::spawn(async move {
        stalled_hello(&server).await?;
        delayed_hello(&server).await
    });
    let (link, mut updates) = link(&host).await?;
    let seen = tokio::time::timeout(STEP, until_live(&mut updates)).await??;
    assert_eq!(
        seen.iter().map(|status| status.state).collect::<Vec<_>>(),
        Vec::from([
            LinkState::Connecting,
            LinkState::Authenticating,
            LinkState::Disconnected,
            LinkState::Connecting,
            LinkState::Authenticating,
            LinkState::CatchingUp,
            LinkState::Live,
        ])
    );
    assert_eq!(
        seen.iter()
            .filter(|status| status.state == LinkState::Disconnected)
            .map(|status| status.error.as_deref())
            .collect::<Vec<_>>(),
        Vec::from([Some(
            "the host did not complete the hello handshake within 10 seconds"
        )])
    );
    assert_eq!(link.status().error, None);
    link.stop().await?;
    serving.await??;
    host.close().await;
    Ok(())
}

#[tokio::test]
async fn slow_hello_succeeds_without_reconnecting() -> TestResult<()> {
    let host = endpoint().await?;
    let server = host.clone();
    let serving = tokio::spawn(async move { delayed_hello(&server).await });
    let (link, mut updates) = link(&host).await?;
    let seen = tokio::time::timeout(STEP, until_live(&mut updates)).await??;
    assert_eq!(
        seen.into_iter()
            .map(|status| status.state)
            .collect::<Vec<_>>(),
        Vec::from([
            LinkState::Connecting,
            LinkState::Authenticating,
            LinkState::CatchingUp,
            LinkState::Live,
        ])
    );
    link.stop().await?;
    serving.await??;
    host.close().await;
    Ok(())
}

#[tokio::test]
async fn stop_completes_while_a_dial_is_pending() -> TestResult<()> {
    let host = endpoint().await?;
    let (link, _updates) = link(&host).await?;
    let incoming = tokio::time::timeout(STEP, host.accept()).await?;
    assert!(incoming.is_some());
    tokio::time::timeout(Duration::from_secs(3), link.stop()).await??;
    drop(incoming);
    host.close().await;
    Ok(())
}

#[tokio::test]
async fn stop_completes_while_a_subscription_is_pending() -> TestResult<()> {
    let host = endpoint().await?;
    let (link, _updates) = link(&host).await?;
    let (conn, mut send) =
        tokio::time::timeout(STEP, accept_hello(&host)).await??;
    welcome(&mut send).await?;
    let _halves =
        tokio::time::timeout(STEP, accept_subscription(&conn)).await??;
    assert_eq!(link.status().state, LinkState::CatchingUp);
    tokio::time::timeout(Duration::from_secs(3), link.stop()).await??;
    host.close().await;
    Ok(())
}
