---
construct: macro.env
status: proposed
task: bootstrap round (docs/subset/bootstrap-report.txt)
---

# `env!` for Cargo metadata

## Rule
Add `env` to `macros` for `CARGO_PKG_VERSION`, `CARGO_PKG_NAME` and `CARGO_MANIFEST_DIR` only. These are the variables Cargo sets at build time. Other environment variables are read at run time with `std::env::var`, which returns a `Result`.

## Before
No subset-only form exists for the package version. `std::env::var("CARGO_PKG_VERSION")` is set only under `cargo run`, not in an installed binary. The Codex handshake sends it as `clientInfo.version`.

## After
```rust
// crates/agents/src/codex/protocol.rs
client_info: ClientInfo {
    name: "conductor".into(),
    title: None,
    version: env!("CARGO_PKG_VERSION").into(),
},

// crates/agents/tests/replay.rs
Path::new(env!("CARGO_MANIFEST_DIR"))
```

## Measurements
| | tokens | lines |
|---|---|---|
| before | n/a | n/a |
| after | n/a | n/a |
| change | n/a | n/a |

Not measured: there is no version without the construct. The proposal rests on necessity.

Friction points removed: 3 `macro.env` sites in 3 files.

## Qualities
1. Named states: kept.
2. Visible branching: kept.
3. Forward data flow: kept. The value is fixed at build time.
4. Errors are values: kept. A missing variable is a build error, not a runtime one.
5. Local reasoning: kept.
6. One way: kept.

## Owner decision
