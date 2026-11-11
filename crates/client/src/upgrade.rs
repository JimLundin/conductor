//! When to move a browser's relayed iroh connection onto WebRTC (§5.4).
//! Native clients hole-punch with iroh and never upgrade: there `negotiate`
//! never resolves, so an attempt never ends.

use std::time::Duration;

use iroh::endpoint::Connection;
use n0_future::boxed::BoxFuture;
use n0_future::time::sleep;

use crate::LinkState;
use crate::link::{backoff, random_u64};
use crate::platform::{Session, negotiate};

const RETRY_FLOOR: Duration = Duration::from_secs(2);
const RETRY_CEILING: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy)]
enum Stage {
    /// The relayed link is not live yet.
    Idle,
    /// An attempt is waiting or running, or WebRTC carries the traffic.
    Started,
}

/// The upgrade of one iroh connection. The link polls `attempt` in its
/// `select!`, so the timer or attempt in progress survives a turn that picks
/// another branch.
pub(crate) struct Upgrade {
    stun_servers: Vec<String>,
    stage: Stage,
    failures: u32,
    /// The timer and attempt in progress, or a future that never resolves.
    pub(crate) attempt: BoxFuture<Result<Session, String>>,
}

impl Upgrade {
    pub(crate) fn new(stun_servers: Vec<String>) -> Upgrade {
        Upgrade {
            stun_servers,
            stage: Stage::Idle,
            failures: 0,
            attempt: Box::pin(std::future::pending()),
        }
    }

    /// Starts the first attempt once the relayed link is live.
    pub(crate) fn arm(self, state: LinkState, conn: &Connection) -> Upgrade {
        match (self.stage, state) {
            (Stage::Idle, LinkState::Live) => Upgrade {
                stage: Stage::Started,
                attempt: Box::pin(negotiate(conn.clone(), self.stun_servers.clone())),
                ..self
            },
            (Stage::Idle | Stage::Started, _) => self,
        }
    }

    /// WebRTC is up: the next failure starts the backoff over.
    pub(crate) fn succeeded(self) -> Upgrade {
        Upgrade {
            failures: 0,
            attempt: Box::pin(std::future::pending()),
            ..self
        }
    }

    /// After an attempt failed or WebRTC dropped: stay on iroh for a while,
    /// then try again.
    pub(crate) fn retry_later(self, conn: &Connection) -> Upgrade {
        let delay = backoff(self.failures, random_u64(), RETRY_FLOOR, RETRY_CEILING);
        tracing::info!(
            retry_in_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
            "webrtc upgrade retry scheduled"
        );
        Upgrade {
            stage: Stage::Started,
            failures: self.failures.saturating_add(1),
            attempt: Box::pin(retry(delay, conn.clone(), self.stun_servers.clone())),
            ..self
        }
    }
}

async fn retry(
    delay: Duration,
    conn: Connection,
    stun_servers: Vec<String>,
) -> Result<Session, String> {
    sleep(delay).await;
    negotiate(conn, stun_servers).await
}
