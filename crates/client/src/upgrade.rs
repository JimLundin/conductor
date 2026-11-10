//! When to move a browser's relayed iroh connection onto WebRTC (§5.4).
//! Native clients hole-punch with iroh and never upgrade, so there
//! [`Upgrade`] never yields.

#[cfg(not(target_family = "wasm"))]
pub use native::Upgrade;
#[cfg(target_family = "wasm")]
pub use web::Upgrade;

#[cfg(not(target_family = "wasm"))]
mod native {
    use iroh::endpoint::Connection;

    use crate::wire::Transport;

    pub struct Upgrade;

    impl Upgrade {
        pub fn new(_stun_servers: &[String]) -> Self {
            Self
        }

        pub fn arm(&mut self) {}

        pub fn retry_later(&mut self) {}

        pub async fn next(&mut self, _conn: &Connection) -> Result<Transport, String> {
            std::future::pending().await
        }
    }
}

#[cfg(target_family = "wasm")]
mod web {
    use std::future::Future;
    use std::pin::Pin;
    use std::time::Duration;

    use iroh::endpoint::Connection;
    use n0_future::time::sleep;

    use crate::platform::rtc;
    use crate::wire::Transport;

    const RETRY_FLOOR: Duration = Duration::from_secs(2);
    const RETRY_CEILING: Duration = Duration::from_secs(120);

    type Attempt = Pin<Box<dyn Future<Output = Result<rtc::Session, String>>>>;

    /// `next` is cancel-safe: the timer or attempt in progress lives here, so
    /// a `select!` that picks another branch loses nothing.
    pub struct Upgrade {
        stun_servers: Vec<String>,
        failures: u32,
        state: State,
    }

    enum State {
        /// The relayed link is not live yet.
        Idle,
        /// WebRTC carries the traffic.
        Up,
        Waiting(Pin<Box<dyn Future<Output = ()>>>),
        Trying(Attempt),
    }

    impl Upgrade {
        pub fn new(stun_servers: &[String]) -> Self {
            Self {
                stun_servers: stun_servers.to_vec(),
                failures: 0,
                state: State::Idle,
            }
        }

        /// Starts the first attempt once the relayed link is live.
        pub fn arm(&mut self) {
            if matches!(self.state, State::Idle) {
                self.state = State::Waiting(Box::pin(std::future::ready(())));
            }
        }

        /// After WebRTC dropped: stay on iroh for a while, then try again.
        pub fn retry_later(&mut self) {
            self.wait();
        }

        pub async fn next(&mut self, conn: &Connection) -> Result<Transport, String> {
            loop {
                match &mut self.state {
                    State::Idle | State::Up => std::future::pending::<()>().await,
                    State::Waiting(timer) => {
                        timer.await;
                        let attempt = rtc::negotiate(conn.clone(), self.stun_servers.clone());
                        self.state = State::Trying(Box::pin(attempt));
                    }
                    State::Trying(attempt) => {
                        let result = attempt.await;
                        return match result {
                            Ok(session) => {
                                self.failures = 0;
                                self.state = State::Up;
                                Ok(Transport::WebRtc(session))
                            }
                            Err(reason) => {
                                self.wait();
                                Err(reason)
                            }
                        };
                    }
                }
            }
        }

        fn wait(&mut self) {
            let delay = crate::link::backoff(
                self.failures,
                crate::link::random_u64(),
                RETRY_FLOOR,
                RETRY_CEILING,
            );
            self.failures = self.failures.saturating_add(1);
            tracing::info!(
                retry_in_ms = delay.as_millis() as u64,
                "webrtc upgrade retry scheduled"
            );
            self.state = State::Waiting(Box::pin(sleep(delay)));
        }
    }
}
