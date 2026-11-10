# Subset patterns by example

What the constructs in `subset.toml` (version 1) look like in practice.
Every example uses only allowed constructs. A **Note** marks where the core
forces a longer form. Those are the likely early proposals.

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HostSeq(u64);

impl HostSeq {
    pub const ZERO: HostSeq = HostSeq(0);

    pub fn next(self) -> Option<HostSeq> {
        self.0.checked_add(1).map(HostSeq)
    }
}
```

There are no `+ - * /` operators. Overflow is a `None` the caller has to
handle.

## 3. Comparison is an `Ordering`, matched

```rust
use std::cmp::Ordering;

pub enum Freshness {
    Behind,
    Current,
    Ahead,
}

impl HostSeq {
    pub fn freshness(self, head: HostSeq) -> Freshness {
        match self.0.cmp(&head.0) {
            Ordering::Less => Freshness::Behind,
            Ordering::Equal => Freshness::Current,
            Ordering::Greater => Freshness::Ahead,
        }
    }
}
```

This replaces `if seen < head { … } else if …`.

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
**Note:** with no trait impls in the core, there's no `impl From` for `?`
to convert through, and no `Display` or `Error` for the type, so every
conversion is an explicit `map_err`.

## 8. Collections are transformed with adapters

```rust
pub fn pending(requests: &Vec<Request>) -> Vec<RequestId> {
    requests
        .iter()
        .filter_map(|request| match request.state {
            RequestState::Open => Some(request.id),
            RequestState::Resolved { .. } => None,
        })
        .collect()
}
```

Use `filter_map` with a `match` rather than `filter` with a condition.
**Note:** `&[Request]` (a slice type) isn't in the core yet, hence
`&Vec`.

## 9. Totals are folds with checked arithmetic

```rust
pub fn total_bytes(frames: &Vec<Frame>) -> Option<u64> {
    frames
        .iter()
        .try_fold(0_u64, |total, frame| total.checked_add(frame.len))
}
```

## 10. Ask for the value, not whether it exists

```rust
pub fn describe(events: &Vec<Event>) -> String {
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
tracing::info!(seq = seen.0, host = %name, "caught up");
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

## Not expressible in the core yet

The bootstrap round decides each of these with a proposal:

- `async` / `.await`, and the actor `loop` around `select!`.
- Trait impls: `Display`, `Error` (thiserror), `From`, `Default`, serde.
- Generic definitions.
- Slices and the unit type `()`.
- Derives beyond the six listed, such as `PartialOrd`, `Ord`, `Default`,
  `Serialize` and `Deserialize`.
