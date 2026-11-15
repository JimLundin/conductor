use std::time::Duration;

use client::{
    HostLink, LinkOptions, LinkState, Network, Status, Target, Update,
};
use iroh::endpoint::{Connection, ConnectionError, SendStream, presets};
use iroh::{Endpoint, RelayMode};
use proto::wire;
use proto::{
    AuthToken, Availability, ClientName, Epoch, HelloReply, HostSeq, Open,
    Providers, Welcome,
};
use tokio::sync::mpsc;

const STEP: Duration = Duration::from_secs(15);

async fn endpoint() -> Endpoint {
    Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .alpns(Vec::from([proto::ALPN.to_vec()]))
        .bind()
        .await
        .unwrap()
}

async fn link(endpoint: &Endpoint) -> (HostLink, mpsc::Receiver<Update>) {
    HostLink::spawn(
        Target {
            addr: endpoint.addr(),
            token: AuthToken::from_bytes([7; 16]),
        },
        LinkOptions {
            network: Network::Local,
            ..LinkOptions::new(ClientName(String::from("handshake-test")))
        },
    )
    .await
    .unwrap()
}

async fn accept_hello(endpoint: &Endpoint) -> (Connection, SendStream) {
    let conn = endpoint.accept().await.unwrap().await.unwrap();
    let (send, mut recv) = conn.accept_bi().await.unwrap();
    let opening = wire::read::<Open>(&mut recv).await.unwrap().unwrap();
    assert_eq!(
        Some(&opening).filter(|opening| matches!(opening, Open::Hello(_))),
        Some(&opening)
    );
    (conn, send)
}

async fn stalled_hello(endpoint: &Endpoint) {
    let (conn, _send) = accept_hello(endpoint).await;
    let closed = tokio::time::timeout(STEP, conn.closed()).await.unwrap();
    assert_eq!(
        Some(&closed).filter(|closed| matches!(
            closed,
            ConnectionError::ApplicationClosed(_)
        )),
        Some(&closed)
    );
}

async fn delayed_hello(endpoint: &Endpoint) {
    let (conn, mut send) = accept_hello(endpoint).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let unavailable = Availability::Unavailable {
        reason: String::from("handshake test"),
    };
    wire::write(
        &mut send,
        &HelloReply::Welcome(Welcome {
            host: String::from("test-host"),
            epoch: Epoch(11),
            head_seq: HostSeq(0),
            providers: Providers {
                claude: unavailable.clone(),
                codex: unavailable,
            },
        }),
    )
    .await
    .unwrap();
    send.finish().unwrap();
    let _ = tokio::time::timeout(STEP, conn.closed()).await.unwrap();
}

async fn until_live(updates: &mut mpsc::Receiver<Update>) -> Vec<Status> {
    let mut seen = Vec::new();
    loop {
        match updates.recv().await.unwrap() {
            Update::Status(status) => {
                seen.push(status.clone());
                if let LinkState::Live = status.state {
                    break seen;
                }
            }
            Update::Reset(_) | Update::Event(_) => {}
        }
    }
}

#[tokio::test]
async fn stalled_hello_closes_the_connection_and_retries() {
    let host = endpoint().await;
    let server = host.clone();
    let serving = tokio::spawn(async move {
        stalled_hello(&server).await;
        delayed_hello(&server).await;
    });
    let (link, mut updates) = link(&host).await;
    let seen = tokio::time::timeout(STEP, until_live(&mut updates))
        .await
        .unwrap();
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
    link.stop().await.unwrap();
    serving.await.unwrap();
    host.close().await;
}

#[tokio::test]
async fn slow_hello_succeeds_without_reconnecting() {
    let host = endpoint().await;
    let server = host.clone();
    let serving = tokio::spawn(async move { delayed_hello(&server).await });
    let (link, mut updates) = link(&host).await;
    let seen = tokio::time::timeout(STEP, until_live(&mut updates))
        .await
        .unwrap();
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
    link.stop().await.unwrap();
    serving.await.unwrap();
    host.close().await;
}
