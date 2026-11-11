# Subset patterns by example

What the constructs in `subset.toml` (version 3) look like in practice.
Every example passes `subset check`, and a test in `tools/subset` keeps it
that way. A first-line comment naming a path, such as
`// crates/proto/src/frame.rs`, places an example where a location rule
allows it. A **Note** marks where the subset forces a longer form.

## 1. Two states are an enum, not a `bool`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Shown,
    Hidden,
}

pub fn toggle(visibility: Visibility) -> Visibility {
    match visibility {
        Visibility::Shown => Visibility::Hidden,
        Visibility::Hidden => Visibility::Shown,
    }
}
```

## 2. IDs and counters are newtypes, and arithmetic is a checked method

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HostSeq(u64);

impl HostSeq {
    pub const ZERO: HostSeq = HostSeq(0);

    pub fn next(self) -> Option<HostSeq> {
        self.0.checked_add(1).map(HostSeq)
    }
}
```

There are no `+ - * /` operators. Overflow is a `None` the caller has to
handle. `HostSeq` derives `PartialOrd` and `Ord` because its order means
something (proposal 0024). An ID such as `RequestId`, whose order means
nothing, doesn't.

## 3. Comparison is an `Ordering`, matched on the whole newtype

```rust
use std::cmp::Ordering;

pub enum Freshness {
    Behind,
    Current,
    Ahead,
}

impl HostSeq {
    pub fn freshness(self, head: HostSeq) -> Freshness {
        match self.cmp(&head) {
            Ordering::Less => Freshness::Behind,
            Ordering::Equal => Freshness::Current,
            Ordering::Greater => Freshness::Ahead,
        }
    }
}
```

This replaces `if seen < head { … } else if …`. Compare the ordered newtype
itself with `a.cmp(&b)`, never its field with `a.0.cmp(&b.0)` (proposal 0024).

## 4. Choosing by value is a literal pattern

```rust
pub enum Provider {
    ClaudeCode,
    Codex,
}

pub fn provider(name: &str) -> Option<Provider> {
    match name {
        "claude" => Some(Provider::ClaudeCode),
        "codex" => Some(Provider::Codex),
        _ => None,
    }
}
```

This replaces `if name == "claude"`.

## 5. A state machine is an enum with a transition function

```rust
pub enum Link {
    Disconnected { attempts: u32 },
    Connecting { attempts: u32 },
    Live { session: SessionId },
}

pub enum LinkEvent {
    Dialled,
    Opened(SessionId),
    Dropped,
}

impl Link {
    pub fn on(self, event: LinkEvent) -> Link {
        match (self, event) {
            (_, LinkEvent::Opened(session)) => Link::Live { session },
            (Link::Disconnected { attempts } | Link::Connecting { attempts }, LinkEvent::Dialled) => {
                Link::Connecting { attempts }
            }
            (Link::Disconnected { attempts }, LinkEvent::Dropped) => Link::Disconnected { attempts },
            (Link::Connecting { attempts }, LinkEvent::Dropped) => Link::Disconnected {
                attempts: attempts.saturating_add(1),
            },
            (Link::Live { session }, LinkEvent::Dialled) => Link::Live { session },
            (Link::Live { .. }, LinkEvent::Dropped) => Link::Disconnected { attempts: 0 },
        }
    }
}
```

The `match` on `(state, event)` reads as a transition table. Tuples were
added in version 1 (proposal 0001).

## 6. Missing values are `Option`, matched

```rust
pub fn label(host_name: Option<&str>) -> String {
    match host_name {
        Some(name) => format!("host {name}"),
        None => String::from("unnamed host"),
    }
}
```

## 7. Errors are enums, passed on with `?`

```rust
#[derive(Debug)]
pub enum LoadError {
    Read(std::io::Error),
    Parse(toml::de::Error),
}

pub fn load(path: &Path) -> Result<Config, LoadError> {
    let text = std::fs::read_to_string(path).map_err(LoadError::Read)?;
    toml::from_str(&text).map_err(LoadError::Parse)
}
```

A variant constructor such as `LoadError::Read` is passed as a function.
`map_err` is the form to use when one error type can mean different
things in the same enum. When it means one thing, use `#[from]` (§16).

## 8. Collections are transformed with adapters

```rust
pub fn pending(requests: &[Request]) -> Vec<RequestId> {
    requests
        .iter()
        .filter_map(|request| match request.state {
            RequestState::Open => Some(request.id),
            RequestState::Resolved { .. } => None,
        })
        .collect()
}
```

Use `filter_map` with a `match` rather than `filter` with a condition. A
function that only reads a sequence takes `&[T]` (proposal 0014).

## 9. Totals are folds with checked arithmetic

```rust
pub fn total_bytes(frames: &[Frame]) -> Option<u64> {
    frames
        .iter()
        .try_fold(0_u64, |total, frame| total.checked_add(frame.len))
}
```

## 10. Ask for the value, not whether it exists

```rust
pub fn describe(events: &[Event]) -> String {
    match events.last() {
        Some(event) => format!("last event #{}", event.seq.0),
        None => String::from("no events yet"),
    }
}
```

This replaces `if events.is_empty() { … } else { events[events.len() - 1] }`.

## 11. When std only gives a `bool`

```rust
pub enum Workspace {
    Present(PathBuf),
    Missing,
}

pub fn workspace(path: PathBuf) -> Workspace {
    match std::fs::metadata(&path) {
        Ok(_) => Workspace::Present(path),
        Err(_) => Workspace::Missing,
    }
}
```

Prefer the `Result`/`Option` form of an API (`metadata` over `exists()`).
Where only a `bool` exists, `.then_some(value)` turns it into an `Option`
to match on. The reviewer treats repeated use as friction.

## 12. A changed value is a new value

```rust
pub fn rename(host: Host, name: String) -> Host {
    Host { name, ..host }
}
```

Use struct update instead of `let mut host = host; host.name = name;`.

## 13. Logging

```rust
pub fn caught_up(seen: HostSeq, name: &str) {
    tracing::info!(seq = seen.0, host = %name, "caught up");
}
```

## 14. Tests compare values

```rust
#[cfg(test)]
mod tests {
    use super::{toggle, Visibility};

    #[test]
    fn toggling_twice_returns_to_the_start() {
        assert_eq!(toggle(toggle(Visibility::Shown)), Visibility::Shown);
    }
}
```

## 15. An actor owns its state and runs one loop

```rust
use std::convert::Infallible;

use tokio::sync::{mpsc, oneshot};

pub enum Msg {
    Append(Event),
    Head { reply: oneshot::Sender<HostSeq> },
}

/// The inbox closed: nobody can reach the actor any more.
#[derive(Debug)]
pub struct Stopped {}

pub struct Log {
    events: Vec<Event>,
    head: HostSeq,
}

impl Log {
    pub async fn run(mut self, mut inbox: mpsc::Receiver<Msg>) -> Result<Infallible, Stopped> {
        loop {
            let msg = inbox.recv().await.ok_or(Stopped {})?;
            self.on(msg);
        }
    }

    fn on(&mut self, msg: Msg) {
        match msg {
            Msg::Append(event) => {
                self.head = event.seq;
                self.events.push(event);
            }
            Msg::Head { reply } => {
                let _ = reply.send(self.head);
            }
        }
    }
}
```

`run` is `async` because it waits on a channel (proposal 0006). The `loop` is
its whole body: it takes one message and hands it to one method (0007). Only
the actor's own state changes, through `mut self` and `&mut self` (0008).
Every other function still builds new values. The reply's send fails only
when the asker has gone, and there is nothing to do about it, so the result
is discarded with `let _ =` (proposal 0025).

## 16. Errors convert with `?` through `#[from]`

```rust
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("reading the config: {0}")]
    Read(#[from] std::io::Error),
    #[error("parsing the config: {0}")]
    Parse(#[from] toml::de::Error),
}

pub fn load_config(path: &Path) -> Result<Config, ConfigError> {
    let text = std::fs::read_to_string(path)?;
    Ok(toml::from_str(&text)?)
}
```

Each variant has one message (proposal 0010). `#[from]` fits because each
source error type appears in one variant only (0011). Where the same type
means two things, use `map_err` as in §7.

## 17. Wire types derive serde

```rust
// crates/proto/src/frame.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HostSeq(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Frame {
    Hello { host: String },
    Event { seq: HostSeq, body: String },
}
```

Only types that cross a wire derive serde, and they live in `crates/proto` or
an agents protocol module (proposal 0012).

## 18. Platform code comes in pairs

```rust
pub enum Platform {
    Browser,
    Native,
}

#[cfg(target_family = "wasm")]
pub fn platform() -> Platform {
    Platform::Browser
}

#[cfg(not(target_family = "wasm"))]
pub fn platform() -> Platform {
    Platform::Native
}
```

A `cfg` sits on a whole item, and the other platform's item sits next to it
in the same file (proposal 0016).

## 19. Discarding a value is `let _ =`

```rust
use tokio::sync::oneshot;

pub fn answer(reply: oneshot::Sender<HostSeq>, head: HostSeq) {
    let _ = reply.send(head);
}
```

`let _ = value;` is the one way to say "this result doesn't matter". Don't
write `drop(reply.send(head))` or `reply.send(head).ok();`. `drop` is only
for releasing a named value that owns a resource earlier than the end of
its scope (proposal 0025, revised).

## 20. An actor handle asks through one `ask` helper

```rust
use tokio::sync::{mpsc, oneshot};

pub enum LogMsg {
    Head { reply: oneshot::Sender<HostSeq> },
    Len { reply: oneshot::Sender<usize> },
}

/// The actor stopped before it answered.
#[derive(Debug)]
pub struct LogStopped {}

pub struct LogHandle {
    commands: mpsc::Sender<LogMsg>,
}

impl LogHandle {
    pub async fn head(&self) -> Result<HostSeq, LogStopped> {
        self.ask(|reply| LogMsg::Head { reply }).await
    }

    pub async fn len(&self) -> Result<usize, LogStopped> {
        self.ask(|reply| LogMsg::Len { reply }).await
    }

    async fn ask<T>(&self, build: impl FnOnce(oneshot::Sender<T>) -> LogMsg) -> Result<T, LogStopped> {
        let (reply, answer) = oneshot::channel();
        self.commands.send(build(reply)).await.map_err(|_| LogStopped {})?;
        answer.await.map_err(|_| LogStopped {})
    }
}
```

Every request and reply round trip goes through `ask`, a private helper of
exactly this shape (proposal 0026). The closure only builds the message
variant. `ask` is the one place outside a codec where a fn takes a type
parameter, and the one `impl Trait` argument in the subset.

## 21. Acting on one case is an `if let` with no `else`

```rust
pub fn announce(host_name: Option<&str>) {
    if let Some(name) = host_name {
        tracing::info!(host = %name, "host named");
    }
}
```

When only one case needs doing and the rest need nothing, write `if let`
with no `else`, no `else if` and no `&&` let-chain (proposal 0027). It
replaces a `match` whose other arm is `None => {}`, which clippy's
`single_match` rejects. As soon as another case needs something, it is a
`match` with every arm written out (§6).

## Still outside the subset

- Generic structs, enums and traits, and any bound other than the codec
  bounds of proposal 0013.
- `Display`, `FromStr` and `From` impls. Write `label`, `parse` and `to_…`
  methods instead (proposal 0009).
- Derives beyond the listed ones, such as `Default`, and `PartialOrd` or
  `Ord` on anything but an ordered newtype.
- `if`, `if let` with an `else` or a let-chain, `while`, `for`, `return`
  and the operators. They stay rewrites into
  the core (`docs/subset/bootstrap-review.md`).
