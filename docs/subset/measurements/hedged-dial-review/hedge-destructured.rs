/// A dial in flight and the endpoint it runs on.
struct Attempt {
    endpoint: Endpoint,
    connecting: BoxFuture<Result<Connection, ConnectError>>,
}

impl Attempt {
    fn start(endpoint: Endpoint, addr: EndpointAddr) -> Attempt {
        Attempt {
            connecting: Box::pin(dial(endpoint.clone(), addr)),
            endpoint,
        }
    }
}

/// How a dial ended, and the endpoint the link runs on from now on.
struct Dialed {
    endpoint: Endpoint,
    conn: Result<Connection, ConnectError>,
}

async fn dial(endpoint: Endpoint, addr: EndpointAddr) -> Result<Connection, ConnectError> {
    endpoint.connect(addr, proto::ALPN).await
}

/// Dials from `endpoint`, and from a fresh endpoint too if the first dial
/// hasn't returned after [`HEDGE_AFTER`].
async fn dial_hedged(endpoint: Endpoint, addr: EndpointAddr) -> Dialed {
    let Attempt {
        endpoint,
        mut connecting,
    } = Attempt::start(endpoint, addr.clone());
    tokio::select! {
        conn = &mut connecting => Dialed { endpoint, conn },
        () = sleep(HEDGE_AFTER) => hedge(Attempt { endpoint, connecting }, addr).await,
    }
}

async fn hedge(first: Attempt, addr: EndpointAddr) -> Dialed {
    tracing::info!("dial stalled; dialing again on a fresh endpoint");
    match platform::bind_endpoint().await {
        Ok(fresh) => race(first, Attempt::start(fresh, addr)).await,
        Err(err) => {
            tracing::warn!(%err, "no fresh endpoint to hedge with");
            finish(first).await
        }
    }
}

/// Keeps whichever attempt connects first.
async fn race(first: Attempt, second: Attempt) -> Dialed {
    let Attempt {
        endpoint: first_endpoint,
        connecting: mut first_connecting,
    } = first;
    let Attempt {
        endpoint: second_endpoint,
        connecting: mut second_connecting,
    } = second;
    tokio::select! {
        conn = &mut first_connecting => {
            let second = Attempt { endpoint: second_endpoint, connecting: second_connecting };
            settle(Dialed { endpoint: first_endpoint, conn }, second).await
        }
        conn = &mut second_connecting => {
            let first = Attempt { endpoint: first_endpoint, connecting: first_connecting };
            settle(Dialed { endpoint: second_endpoint, conn }, first).await
        }
    }
}

/// One attempt returned. If it connected, the other is dropped and its
/// endpoint closed; if it failed, its endpoint is closed and the other
/// decides.
async fn settle(done: Dialed, other: Attempt) -> Dialed {
    match done.conn {
        Ok(conn) => {
            tracing::info!(endpoint = %done.endpoint.id().fmt_short(), "hedged dial connected");
            retire(other.endpoint);
            Dialed {
                endpoint: done.endpoint,
                conn: Ok(conn),
            }
        }
        Err(err) => {
            tracing::debug!(%err, "one hedged dial failed");
            retire(done.endpoint);
            finish(other).await
        }
    }
}

async fn finish(attempt: Attempt) -> Dialed {
    Dialed {
        conn: attempt.connecting.await,
        endpoint: attempt.endpoint,
    }
}

/// Closes an endpoint the link no longer uses, without waiting for it.
fn retire(endpoint: Endpoint) {
    task::spawn(async move { endpoint.close().await });
}
