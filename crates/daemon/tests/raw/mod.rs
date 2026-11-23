//! A client that speaks the client link's frames itself, with no fold: its
//! tests see each JSON frame as the host sent it and its decoded `proto`
//! value. A scenario using it records every frame its clients receive and
//! writes them as its feed.
//!
//! A feed, `fixtures/feeds/<scenario>.json`, is an array in receive order
//! of `{"connection", "stream", "frame"}`: the connection's number, counting
//! from 0 in the order they opened; `hello`, `command` or `feed`; and the
//! `HelloReply`, `CommandResult` or `Feed` body. Session ids are numbered
//! `SessionId(1)` up and epochs `1` up, in the order they first appear, and
//! the paths are those of a checkout at `/conductor` run as process 0.
//! `tools/check-feeds.py` fails when a committed feed differs from what its
//! scenario writes.

use std::path::PathBuf;

use iroh::Endpoint;
use iroh::endpoint::{Connection, RecvStream};
use net::Target;
use proto::{
    ClientName, Command, CommandBody, CommandId, CommandResult, Epoch, Event,
    Feed, Frame, Hello, HelloReply, Home, HostSeq, Instances, Open, SessionId,
    Subscribe, Welcome,
};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::{Fallible, STEP, endpoint, started_session, target_tmp};

/// One frame as it came, and what it decodes to.
pub struct Received<T> {
    pub raw: Vec<u8>,
    pub value: T,
}

/// A frame a client received, in the order its clients received them.
struct Heard {
    connection: usize,
    stream: &'static str,
    raw: Vec<u8>,
}

/// The frames a scenario's clients received.
pub struct Recorder {
    scenario: String,
    connections: usize,
    hosts: Vec<String>,
    heard: mpsc::UnboundedSender<Heard>,
    feed: mpsc::UnboundedReceiver<Heard>,
}

/// Where one connection's frames go.
struct Tap {
    connection: usize,
    heard: mpsc::UnboundedSender<Heard>,
}

impl Tap {
    fn hear(&self, stream: &'static str, raw: &[u8]) -> Fallible {
        self.heard
            .send(Heard {
                connection: self.connection,
                stream,
                raw: raw.to_vec(),
            })
            .map_err(|_| "the recorder is gone")?;
        Ok(())
    }
}

/// The ids a feed has seen, each to be replaced by a fixed one.
#[derive(Default)]
struct Recording {
    sessions: Vec<String>,
    epochs: Vec<String>,
    hosts: Vec<String>,
    commits: Vec<String>,
}

impl Recorder {
    /// A recorder named after the running test.
    pub fn new() -> Fallible<Self> {
        let scenario = std::thread::current()
            .name()
            .ok_or("the test thread has no name")?
            .replace("::", ".");
        let (heard, feed) = mpsc::unbounded_channel();
        Ok(Self {
            scenario,
            connections: 0,
            hosts: Vec::new(),
            heard,
            feed,
        })
    }

    /// The next connection, to `host`.
    fn connect(&mut self, host: &Target) -> Tap {
        let id = host.addr.id.to_string();
        if !self.hosts.contains(&id) {
            self.hosts.push(id);
        }
        let connection = self.connections;
        self.connections = connection.saturating_add(1);
        Tap {
            connection,
            heard: self.heard.clone(),
        }
    }

    /// Writes the feed, normalised so that a rerun writes the same bytes,
    /// to `$CONDUCTOR_FEEDS/<scenario>.json`, by default in `fixtures/feeds`.
    pub fn write(mut self) -> Fallible {
        let mut recording = Recording {
            hosts: self.hosts,
            ..Recording::default()
        };
        let mut frames = Vec::new();
        while let Ok(heard) = self.feed.try_recv() {
            let body = serde_json::from_slice::<Value>(&heard.raw)?;
            frames.push(serde_json::json!({
                "connection": heard.connection,
                "stream": heard.stream,
                "frame": recording.normalised(body)?,
            }));
        }
        let dir = std::env::var_os("CONDUCTOR_FEEDS").map_or_else(
            || {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../fixtures/feeds")
            },
            PathBuf::from,
        );
        std::fs::create_dir_all(&dir)?;
        let mut text = serde_json::to_string_pretty(&frames)?;
        text.push('\n');
        std::fs::write(dir.join(format!("{}.json", self.scenario)), text)?;
        Ok(())
    }
}

impl Recording {
    /// `value` with each session id, epoch, host id, commit and test path
    /// replaced by a fixed one, numbered in the order they first appear.
    fn normalised(&mut self, mut value: Value) -> Fallible<Value> {
        self.learn(&value);
        let mut replacements = Vec::new();
        for (seen, n) in self.sessions.iter().zip(1u128..) {
            let fixed = session_text(n)?;
            replacements
                .push((seen.to_ascii_uppercase(), fixed.to_ascii_uppercase()));
            replacements.push((seen.clone(), fixed));
        }
        for (seen, n) in self.hosts.iter().zip(1u128..) {
            replacements.push((seen.clone(), format!("{n:064x}")));
        }
        for (seen, n) in self.commits.iter().zip(1u128..) {
            replacements.push((seen.clone(), format!("{n:040x}")));
        }
        replacements.extend(test_paths()?);
        self.rewrite(&mut value, None, &replacements);
        Ok(value)
    }

    fn learn(&mut self, value: &Value) {
        match value {
            Value::Object(fields) => {
                for (key, field) in fields {
                    let ids = match key.as_str() {
                        "session" => Some(&mut self.sessions),
                        "epoch" => Some(&mut self.epochs),
                        _ => None,
                    };
                    if let (Some(seen), Value::String(text)) = (ids, field)
                        && !seen.contains(text)
                    {
                        seen.push(text.clone());
                    }
                    self.learn(field);
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.learn(item);
                }
            }
            Value::String(text)
                if is_commit(text) && !self.commits.contains(text) =>
            {
                self.commits.push(text.clone());
            }
            _ => {}
        }
    }

    fn rewrite(
        &self,
        value: &mut Value,
        key: Option<&str>,
        replacements: &[(String, String)],
    ) {
        match value {
            Value::Object(fields) => {
                for (name, field) in fields {
                    self.rewrite(field, Some(name), replacements);
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.rewrite(item, None, replacements);
                }
            }
            Value::String(text) => {
                if key == Some("epoch")
                    && let Some(at) =
                        self.epochs.iter().position(|seen| seen == text)
                {
                    *text = at.saturating_add(1).to_string();
                }
                for (from, to) in replacements {
                    if text.contains(from.as_str()) {
                        *text = text.replace(from.as_str(), to);
                    }
                }
            }
            _ => {}
        }
    }
}

fn is_commit(text: &str) -> bool {
    text.len() == 40
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn session_text(n: u128) -> Fallible<String> {
    match serde_json::to_value(SessionId(n))? {
        Value::String(text) => Ok(text),
        other => Err(format!("a session id serialized as {other}").into()),
    }
}

/// The paths of this checkout and this run, as a checkout at `/conductor`
/// running as process 0 has them.
fn test_paths() -> Fallible<Vec<(String, String)>> {
    let tmp = target_tmp()?.display().to_string();
    let pid = std::process::id();
    Ok(Vec::from([
        (
            format!("{tmp}/conductor-e2e-{pid}-"),
            String::from("/conductor/target/tmp/conductor-e2e-0-"),
        ),
        (
            format!("{tmp}/conductor-test-{pid}-"),
            String::from("/conductor/target/tmp/conductor-test-0-"),
        ),
        (tmp, String::from("/conductor/target/tmp")),
        (
            String::from(env!("CARGO_MANIFEST_DIR")),
            String::from("/conductor/crates/daemon"),
        ),
    ]))
}

/// One connection to a host, opened with `Hello`.
pub struct RawClient {
    tap: Tap,
    endpoint: Endpoint,
    conn: Connection,
    pub welcome: Welcome,
    /// The last `Feed::Instances` read, at first `Welcome`'s.
    pub instances: Instances,
    /// The last `Feed::Home` read, at first `Welcome`'s.
    pub home: Option<Home>,
    feed: Option<RecvStream>,
}

impl RawClient {
    /// Dials `target` as `client`.
    pub async fn open(
        target: &Target,
        client: &str,
        recorder: &mut Recorder,
    ) -> Fallible<Self> {
        let endpoint = endpoint().await?;
        let conn = endpoint.connect(target.addr.clone(), proto::ALPN).await?;
        let (mut send, mut recv) = conn.open_bi().await?;
        net::wire::write(
            &mut send,
            &Open::Hello(Hello {
                client: ClientName(client.to_owned()),
                token: target.token,
            }),
        )
        .await?;
        send.finish()?;
        let tap = recorder.connect(target);
        let reply = read::<HelloReply>(&mut recv, &tap, "hello").await?;
        tap.hear("hello", &reply.raw)?;
        let welcome = match reply.value {
            HelloReply::Welcome(welcome) => welcome,
            HelloReply::Refused { reason } => return Err(reason.into()),
        };
        Ok(Self {
            tap,
            endpoint,
            conn,
            instances: welcome.instances.clone(),
            home: welcome.home.clone(),
            welcome,
            feed: None,
        })
    }

    /// Opened, subscribed from the start of the host's log, and caught up.
    pub async fn live(
        target: &Target,
        name: &str,
        recorder: &mut Recorder,
    ) -> Fallible<Self> {
        let mut client = Self::open(target, name, recorder).await?;
        client.subscribe(HostSeq(0)).await?;
        client.caught_up().await?;
        Ok(client)
    }

    pub async fn subscribe(&mut self, after_seq: HostSeq) -> Fallible {
        self.subscribe_in(self.welcome.epoch, after_seq).await
    }

    /// Subscribes with a cursor into `epoch`, which may not be the host's.
    pub async fn subscribe_in(
        &mut self,
        epoch: Epoch,
        after_seq: HostSeq,
    ) -> Fallible {
        let (mut send, recv) = self.conn.open_bi().await?;
        net::wire::write(
            &mut send,
            &Open::Subscribe(Subscribe { epoch, after_seq }),
        )
        .await?;
        send.finish()?;
        self.feed = Some(recv);
        Ok(())
    }

    /// The next frame of the subscription.
    pub async fn next(&mut self) -> Fallible<Received<Feed>> {
        let feed = self.feed.as_mut().ok_or("not subscribed")?;
        let frame = read::<Feed>(feed, &self.tap, "feed").await?;
        self.tap.hear("feed", &frame.raw)?;
        match &frame.value {
            Feed::Instances(instances) => self.instances = instances.clone(),
            Feed::Home(home) => self.home = Some(home.clone()),
            Feed::Event(_) | Feed::SignIn(_) => {}
        }
        Ok(frame)
    }

    /// The frames through the roster that follows the replayed events.
    pub async fn caught_up(&mut self) -> Fallible<Vec<Received<Feed>>> {
        let mut frames = Vec::new();
        loop {
            let frame = self.next().await?;
            let roster = matches!(frame.value, Feed::Instances(_));
            frames.push(frame);
            if roster {
                return Ok(frames);
            }
        }
    }

    /// The events of the subscription through the first that is `done`.
    pub async fn events_until(
        &mut self,
        done: impl Fn(&Event) -> bool,
    ) -> Fallible<Vec<Event>> {
        let mut events = Vec::new();
        loop {
            if let Feed::Event(event) = self.next().await?.value {
                let reached = done(&event);
                events.push(event);
                if reached {
                    return Ok(events);
                }
            }
        }
    }

    /// Sends `body` on a stream of its own and reads its result.
    pub async fn send(
        &self,
        body: CommandBody,
    ) -> Fallible<Received<CommandResult>> {
        let mut id = [0; 16];
        getrandom::fill(&mut id)?;
        let (mut send, mut recv) = self.conn.open_bi().await?;
        let command = Command {
            id: CommandId(u128::from_be_bytes(id)),
            body,
        };
        net::wire::write(&mut send, &Open::Command(command)).await?;
        send.finish()?;
        let result =
            read::<CommandResult>(&mut recv, &self.tap, "command").await?;
        self.tap.hear("command", &result.raw)?;
        Ok(result)
    }

    /// Sends `body` and checks that the host accepted it.
    pub async fn accepted(&self, body: CommandBody) -> Fallible {
        assert_eq!(self.send(body).await?.value, CommandResult::Accepted);
        Ok(())
    }

    /// Sends `body` and returns the session it started.
    pub async fn started(&self, body: CommandBody) -> Fallible<SessionId> {
        started_session(&self.send(body).await?.value)
    }

    pub async fn close(self) {
        self.conn.close(0u32.into(), b"done");
        self.endpoint.close().await;
    }
}

/// The next frame on `recv`, the `stream` stream of the tapped connection.
async fn read<T: Frame>(
    recv: &mut RecvStream,
    tap: &Tap,
    stream: &str,
) -> Fallible<Received<T>> {
    let raw = tokio::time::timeout(
        STEP,
        net::wire::read_body(recv, usize::MAX),
    )
    .await
    .map_err(|_| {
        format!(
            "no {stream} frame on connection {} within the step timeout",
            tap.connection
        )
    })??
    .ok_or("the host finished the stream")?;
    let value = proto::decode(&raw)?;
    Ok(Received { raw, value })
}
