//! Serves the protocol (`proto::ALPN`) on accepted iroh connections. The
//! protocol's rules are [`host::serve`]; this reads and writes the frames
//! and calls the log and the registry.

use host::serve::{self, Admission, Ledger, Order, Relaunch, Route, Stream};
use iroh::endpoint::{
    ClosedStream, Connection, ConnectionError, RecvStream, SendStream,
};
use iroh::protocol::{AcceptError, ProtocolHandler};
use n0_future::time::Instant;
use net::wire::{self, WireError};
use proto::{
    AuthToken, ClientName, Command, CommandResult, Effort, Event, EventKind,
    Feed as Frame, HelloReply, Home, HostSeq, InstanceId, ModelId, Open,
    Rejection, SessionId, SignIn, StartAt, Subscribe, Welcome,
};
use tokio::sync::{broadcast, watch};

use crate::Stopped;
use crate::home::{HomeClone, Placement};
use crate::log::{BeginCommand, Log};
use crate::registry::Registry;

#[derive(Debug, Clone)]
pub struct Conductor {
    log: Log,
    registry: Registry,
    home: HomeClone,
    /// A placeholder for architecture §4's device certificates: every client
    /// must present the token from the ticket.
    token: AuthToken,
}

#[derive(Debug, thiserror::Error)]
enum StreamError {
    #[error("{0}")]
    Wire(#[from] WireError),
    #[error("could not finish the stream: {0}")]
    Finish(#[from] ClosedStream),
    #[error("{0}")]
    Stopped(#[from] Stopped),
    #[error("{0}")]
    Connection(#[from] ConnectionError),
}

/// What a subscription does next.
enum Upcoming {
    Frame(Frame),
    /// Nothing to send: the home changed to none.
    Quiet,
    /// The client fell behind or the log or the registry stopped: end the
    /// stream, and the client resubscribes from its cursor.
    Resubscribe,
    /// The connection is gone.
    Gone,
}

/// A gap-free subscription (architecture §8): the replay and the sign-ins
/// running, then the live feed and the sign-ins' steps. The log attaches
/// its feed in the same turn as it takes the replay, and the registry its
/// steps in the turn it lists the sign-ins, so neither overlaps or leaves
/// a gap.
struct Feed {
    send: SendStream,
    first: std::vec::IntoIter<Frame>,
    live: broadcast::Receiver<Event>,
    sign_ins: broadcast::Receiver<SignIn>,
    home: watch::Receiver<Option<Home>>,
}

impl ProtocolHandler for Conductor {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let peer = conn.remote_id();
        if let Some(client) = self
            .authenticate(&conn)
            .await
            .map_err(AcceptError::from_err)?
        {
            tracing::info!(%peer, client = client.0, "client connected");
            let err = self.streams(&conn, &client).await;
            tracing::info!(%peer, client = client.0, %err, "client disconnected");
        } else {
            tracing::info!(%peer, "refused a client");
            conn.close(1u32.into(), b"refused");
        }
        Ok(())
    }
}

impl Conductor {
    /// The host's protocol handler, which also notes the progress of its
    /// sessions on issues from now on.
    #[must_use]
    pub fn new(
        log: Log,
        registry: Registry,
        home: HomeClone,
        token: AuthToken,
    ) -> Self {
        crate::progress::spawn(log.clone(), home.clone());
        Self {
            log,
            registry,
            home,
            token,
        }
    }

    /// Answers the connection's first stream. Returns the client's name, or
    /// `None` if it was refused.
    async fn authenticate(
        &self,
        conn: &Connection,
    ) -> Result<Option<ClientName>, StreamError> {
        let started = Instant::now();
        let peer = conn.remote_id();
        let connection = conn.stable_id();
        tracing::debug!(%peer, connection, "waiting for hello");
        let (mut send, mut recv) = conn.accept_bi().await?;
        let first = wire::read::<Open>(&mut recv).await?;
        match serve::admit(first, &self.token) {
            Admission::Admitted(client) => {
                tracing::debug!(%peer, connection, elapsed_ms = started.elapsed().as_millis(), "hello received");
                wire::write(
                    &mut send,
                    &HelloReply::Welcome(self.welcome(conn).await?),
                )
                .await?;
                send.finish()?;
                tracing::debug!(%peer, connection, elapsed_ms = started.elapsed().as_millis(), "welcome sent");
                Ok(Some(client))
            }
            Admission::Refused { reason } => {
                wire::write(&mut send, &HelloReply::Refused { reason }).await?;
                send.finish()?;
                // Let the refusal reach the client before the close does.
                let _ = send.stopped().await;
                Ok(None)
            }
        }
    }

    async fn welcome(&self, conn: &Connection) -> Result<Welcome, Stopped> {
        let started = Instant::now();
        let peer = conn.remote_id();
        let connection = conn.stable_id();
        let instances = self.registry.instances().await?;
        tracing::debug!(%peer, connection, elapsed_ms = started.elapsed().as_millis(), "welcome instances ready");
        let welcome = self
            .log
            .welcome(instances, self.registry.catalog(), self.home.shown())
            .await?;
        tracing::debug!(%peer, connection, elapsed_ms = started.elapsed().as_millis(), "welcome log ready");
        Ok(welcome)
    }

    async fn streams(
        &self,
        conn: &Connection,
        client: &ClientName,
    ) -> ConnectionError {
        loop {
            match conn.accept_bi().await {
                Ok((send, recv)) => {
                    let stream = self.clone().stream(
                        conn.clone(),
                        send,
                        recv,
                        client.clone(),
                    );
                    tokio::spawn(async move {
                        if let Err(err) = stream.await {
                            tracing::debug!(%err, "stream ended with an error");
                        }
                    });
                }
                Err(err) => break err,
            }
        }
    }

    async fn stream(
        self,
        conn: Connection,
        mut send: SendStream,
        mut recv: RecvStream,
        client: ClientName,
    ) -> Result<(), StreamError> {
        match serve::stream(wire::read::<Open>(&mut recv).await?) {
            Stream::Command(command) => {
                tracing::info!(client = client.0, ?command, "command");
                let result = self.run(command, client).await?;
                wire::write(&mut send, &result).await?;
                send.finish()?;
            }
            Stream::Subscribe(subscribe) => {
                self.feed(&conn, send, subscribe).await?;
            }
            Stream::Finish => send.finish()?,
        }
        Ok(())
    }

    /// Runs the command once: a retry answers the first run's receipt.
    async fn run(
        &self,
        command: Command,
        client: ClientName,
    ) -> Result<CommandResult, Stopped> {
        let route = serve::route(command.body, client);
        let begun = match route.ledger() {
            Ledger::Unrecorded => return self.command(route).await,
            Ledger::Session => self.log.begin(command.id).await?,
            Ledger::Receipt => self.log.begin_receipt(command.id).await?,
        };
        match begun {
            BeginCommand::Cached(result) => Ok(result),
            BeginCommand::Execute(lease) => {
                let result = self.command(route).await?;
                lease.finish(result.clone()).await?;
                Ok(result)
            }
        }
    }

    async fn command(&self, route: Route) -> Result<CommandResult, Stopped> {
        match route {
            Route::Order(session, order) => self.order(session, order).await,
            Route::Relaunch(session, relaunch) => {
                self.relaunch(session, relaunch).await
            }
            Route::Start {
                instance,
                at,
                choice,
                by,
            } => self.start(instance, at, choice, by).await,
            Route::Configure(configure) => {
                self.registry.configure(configure).await
            }
            Route::SignIn(order) => self.registry.sign_in(order).await,
            Route::Home(order) => self.home.order(order).await,
            Route::Finish(session, by) => self.finish(session, by).await,
        }
    }

    /// Starts a session in a directory, or in a project's checkout.
    async fn start(
        &self,
        instance: InstanceId,
        at: StartAt,
        choice: (Option<ModelId>, Option<Effort>),
        by: ClientName,
    ) -> Result<CommandResult, Stopped> {
        match self.home.place(at).await? {
            Err(rejection) => Ok(CommandResult::Rejected(rejection)),
            Ok(placed) => self.start_placed(instance, placed, choice, by).await,
        }
    }

    /// Starts a session where it was placed. One on an issue first needs
    /// the instance to have no session on it and the issue's branch to be
    /// ready, and then moves the issue on.
    async fn start_placed(
        &self,
        instance: InstanceId,
        placed: Placement,
        choice: (Option<ModelId>, Option<Effort>),
        by: ClientName,
    ) -> Result<CommandResult, Stopped> {
        let issue = placed.on.as_ref().map(|on| on.issue);
        if let Some(refused) =
            self.unready(&instance, placed.on.as_ref()).await?
        {
            return Ok(CommandResult::Rejected(refused));
        }
        let started = self
            .registry
            .start(instance, placed, choice, by.clone())
            .await?;
        if let (CommandResult::Started { .. }, Some(on)) = (&started, issue) {
            self.home.started(on, by).await?;
        }
        Ok(started)
    }

    /// Why a session on the issue can't start yet: the instance has one on
    /// it, or the issue's branch can't be readied.
    async fn unready(
        &self,
        instance: &InstanceId,
        on: Option<&home::issue::Start>,
    ) -> Result<Option<Rejection>, Stopped> {
        let Some(start) = on else {
            return Ok(None);
        };
        let history = self.log.history().await?;
        if let Some(session) =
            host::registry::on_issue(&history, start.issue, instance)
        {
            return Ok(Some(Rejection::IssueTaken { session }));
        }
        Ok(self.home.ready(start.clone()).await?.err())
    }

    /// Finishes the session in a task of its own, once it can: the
    /// command answers before the agent's summary turn ends, and
    /// `FinishStarted` in the log shows it has begun.
    async fn finish(
        &self,
        session: SessionId,
        by: ClientName,
    ) -> Result<CommandResult, Stopped> {
        let history = self.log.history().await?;
        let finishing = match host::finish::finishing(&history, session) {
            Err(rejection) => return Ok(CommandResult::Rejected(rejection)),
            Ok(finishing) => finishing,
        };
        let Some(handle) = self.registry.find(session).await? else {
            return Ok(CommandResult::Rejected(Rejection::UnknownSession));
        };
        self.log.append(session, EventKind::FinishStarted).await?;
        self.spawn_finish(session, finishing, handle, by);
        Ok(CommandResult::Accepted)
    }

    fn spawn_finish(
        &self,
        session: SessionId,
        finishing: host::finish::Finishing,
        handle: crate::session::Session,
        by: ClientName,
    ) {
        let finisher = self.clone();
        tokio::spawn(async move {
            let ended = finisher.finished(session, finishing, handle, by).await;
            if ended.is_err() {
                tracing::warn!(?session, "finishing stopped with the host");
            }
        });
    }

    /// Summarizes and stops the session, readies the issue's branch from
    /// the remote's, lands and pushes it, notes the issue and moves it on,
    /// then logs what it did. A summary that didn't come, or an agent
    /// that didn't stop, lands nothing.
    async fn finished(
        &self,
        session: SessionId,
        finishing: host::finish::Finishing,
        handle: crate::session::Session,
        by: ClientName,
    ) -> Result<(), Stopped> {
        let summary = self.summary(session, &finishing, &handle, by).await?;
        let branch =
            home::issue::branch(self.home.shown().as_ref(), finishing.issue);
        let finished = match &summary {
            Err(reason) => host::finish::finished(Err(reason.clone()), None),
            Ok(_) => land(&finishing.workspace, &branch).await,
        };
        let signer = ClientName(host::finish::signer(&finishing.instance));
        let commits = host::finish::landed(&finished);
        self.home
            .finished(finishing.issue, summary.ok().flatten(), signer, commits)
            .await?;
        self.log
            .append(session, EventKind::SessionFinished { finished })
            .await
    }

    /// The agent's summary: asked for in one more turn, then the agent is
    /// stopped; the last reply when it had stopped already. Why finishing
    /// can't go on, when the agent wasn't asked, didn't answer in time, or
    /// didn't stop.
    async fn summary(
        &self,
        session: SessionId,
        finishing: &host::finish::Finishing,
        handle: &crate::session::Session,
        by: ClientName,
    ) -> Result<Result<Option<String>, String>, Stopped> {
        let history = self.log.history().await?;
        if !finishing.running {
            return Ok(Ok(host::finish::summary(
                &history,
                session,
                HostSeq(0),
            )));
        }
        let after = history.last().map_or(HostSeq(0), |event| event.seq);
        self.asked_summary(session, handle, by, after).await
    }

    /// Asks the running agent for its summary after `after`, waits for the
    /// turn to end, and stops the agent.
    async fn asked_summary(
        &self,
        session: SessionId,
        handle: &crate::session::Session,
        by: ClientName,
        after: HostSeq,
    ) -> Result<Result<Option<String>, String>, Stopped> {
        let watched = self.log.subscribe(after).await?;
        let text = String::from(host::finish::SUMMARY);
        let asked = handle.order(Order::Send { text, by }).await.ok();
        if let Some(reason) =
            host::finish::interrupted("asking for a summary", asked.as_ref())
        {
            return Ok(Err(reason));
        }
        let Some(seen) = self.turn(session, watched, after).await? else {
            let _ = handle.order(Order::Interrupt).await;
            return Ok(Err(String::from(host::finish::IMPATIENT)));
        };
        let reason = String::from(host::finish::FINISHED);
        let stopped = handle.order(Order::Stop(reason)).await.ok();
        Ok(
            host::finish::interrupted("stopping the agent", stopped.as_ref())
                .map_or_else(
                    || Ok(host::finish::summary(&seen, session, after)),
                    Err,
                ),
        )
    }

    /// The events from `after` until the session's turn ends, or `None`
    /// when it doesn't end within `host::finish::PATIENCE`.
    async fn turn(
        &self,
        session: SessionId,
        watched: crate::log::Subscription,
        after: HostSeq,
    ) -> Result<Option<Vec<Event>>, Stopped> {
        let patience = std::time::Duration::from_secs(host::finish::PATIENCE);
        tokio::time::timeout(
            patience,
            self.until_ended(session, watched, after),
        )
        .await
        .map_or(Ok(None), |seen| seen.map(Some))
    }

    /// Reads the log until the session's turn ends; a subscription that
    /// falls behind reads the log again rather than miss the end.
    async fn until_ended(
        &self,
        session: SessionId,
        watched: crate::log::Subscription,
        after: HostSeq,
    ) -> Result<Vec<Event>, Stopped> {
        let mut seen = watched.replay;
        let mut live = watched.live;
        while !seen
            .iter()
            .any(|event| host::finish::turn_ended(event, session))
        {
            match live.recv().await {
                Ok(event) => seen.push(event),
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let again = self.log.subscribe(after).await?;
                    (seen, live) = (again.replay, again.live);
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(Stopped {});
                }
            }
        }
        Ok(seen)
    }

    async fn order(
        &self,
        session: SessionId,
        order: Order,
    ) -> Result<CommandResult, Stopped> {
        match self.registry.find(session).await? {
            None => Ok(CommandResult::Rejected(Rejection::UnknownSession)),
            Some(handle) => Ok(handle
                .order(order)
                .await
                .unwrap_or(CommandResult::Rejected(Rejection::SessionClosed))),
        }
    }

    async fn relaunch(
        &self,
        session: SessionId,
        relaunch: Relaunch,
    ) -> Result<CommandResult, Stopped> {
        match self.registry.relaunch(session).await? {
            Err(rejection) => Ok(CommandResult::Rejected(rejection)),
            Ok((handle, offered)) => handle
                .order(relaunch.order(offered))
                .await
                .or_else(|stopped| relaunch.closed().ok_or(stopped)),
        }
    }

    async fn feed(
        &self,
        conn: &Connection,
        mut send: SendStream,
        subscribe: Subscribe,
    ) -> Result<(), StreamError> {
        match self.log.resume(subscribe).await? {
            Some(subscription) => {
                tracing::info!(
                    after_seq = subscribe.after_seq.0,
                    replayed = subscription.replay.len(),
                    "subscribed"
                );
                let (running, sign_ins) = self.registry.watch().await?;
                let mut home = self.home.watch();
                let current = home.borrow_and_update().clone();
                let first = subscription
                    .replay
                    .into_iter()
                    .map(Frame::Event)
                    .chain(running.into_iter().map(Frame::SignIn))
                    .chain(current.into_iter().map(Frame::Home))
                    .collect::<Vec<Frame>>();
                let feed = Feed {
                    send,
                    first: first.into_iter(),
                    live: subscription.live,
                    sign_ins,
                    home,
                };
                let result = feed.run(conn).await;
                tracing::info!(?result, "subscription ended");
                result
            }
            // A cursor from another epoch is refused by closing the stream.
            None => Ok(send.finish()?),
        }
    }
}

impl Feed {
    async fn run(mut self, conn: &Connection) -> Result<(), StreamError> {
        loop {
            match self.next(conn).await {
                Upcoming::Frame(frame) => {
                    wire::write(&mut self.send, &frame).await?;
                }
                Upcoming::Quiet => {}
                Upcoming::Resubscribe => break Ok(self.send.finish()?),
                Upcoming::Gone => break Ok(()),
            }
        }
    }

    async fn next(&mut self, conn: &Connection) -> Upcoming {
        if let Some(frame) = self.first.next() {
            Upcoming::Frame(frame)
        } else {
            tokio::select! {
                next = self.live.recv() => upcoming(next.map(Frame::Event)),
                next = self.sign_ins.recv() => upcoming(next.map(Frame::SignIn)),
                changed = self.home.changed() => changed.map_or(
                    Upcoming::Resubscribe,
                    |()| self.home.borrow_and_update().clone().map_or(Upcoming::Quiet, |home| Upcoming::Frame(Frame::Home(home))),
                ),
                // Without this, an idle feed would outlive its connection.
                _ = conn.closed() => Upcoming::Gone,
            }
        }
    }
}

/// Brings the issue's branch up to the remote's, lands the session's on
/// it and pushes it.
async fn land(workspace: &proto::Workspace, branch: &str) -> proto::Finished {
    let checkout = std::path::Path::new(&workspace.repository.0);
    let landed = match crate::home::ready_issue_branch(checkout, branch).await {
        Err(reason) => Err(reason),
        Ok(()) => crate::workspace::land(workspace, branch).await,
    };
    let pushed = match &landed {
        Ok(_) => crate::workspace::push_branch(workspace, branch).await,
        Err(_) => None,
    };
    host::finish::finished(landed, pushed)
}

/// A live frame, or a resubscribe if the subscriber fell behind or the
/// sender stopped.
fn upcoming(next: Result<Frame, broadcast::error::RecvError>) -> Upcoming {
    next.map_or(Upcoming::Resubscribe, Upcoming::Frame)
}
