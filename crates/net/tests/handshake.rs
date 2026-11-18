#![cfg(test)]

use std::time::Duration;

use iroh::endpoint::{Connection, ConnectionError, SendStream, presets};
use iroh::{Endpoint, RelayMode};
use net::wire;
use net::{HostLink, LinkOptions, LinkState, Network, Status, Target, Update};
use proto::{
    AuthToken, Availability, ClientName, Epoch, HelloReply, HostSeq,
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
    let unavailable = InstanceStatus {
        label: String::from("Agent"),
        provider: Provider::Claude,
        availability: Availability::Unavailable {
            reason: String::from("handshake test"),
        },
    };
    wire::write(
        &mut send,
        &HelloReply::Welcome(Welcome {
            host: String::from("test-host"),
            epoch: Epoch(11),
            head_seq: HostSeq(0),
            instances: Instances(
                [(InstanceId::named("agent"), unavailable)].into(),
            ),
            catalog: proto::Catalog::default(),
        }),
    )
    .await?;
    send.finish()?;
    let _ = tokio::time::timeout(STEP, conn.closed()).await?;
    Ok(())
}

async fn until_live(
    updates: &mut mpsc::Receiver<Update>,
) -> TestResult<Vec<Status>> {
    let mut seen = Vec::new();
    loop {
        match updates.recv().await.ok_or("the link's updates ended")? {
            Update::Status(status) => {
                seen.push(status.clone());
                if status.state == LinkState::Live {
                    break Ok(seen);
                }
            }
            Update::Reset(_) | Update::Event(_) => {}
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
