---
construct: generics.type_param, type.impl_trait.arg, expr.closure (one request helper per actor handle)
status: proposed
task: rewrite of crates/agents (rw-agents, friction rows C4, Y1)
---

# One `ask` helper per actor handle

## Rule
An actor's handle may have one private helper that does the request and reply round trip. It is written in exactly this shape:

```rust
async fn ask<T>(&self, build: impl FnOnce(oneshot::Sender<T>) -> Msg) -> Result<T, E> {
    let (reply, answer) = oneshot::channel();
    self.commands.send(build(reply)).await.map_err(|_| E::…)?;
    answer.await.map_err(|_| E::…)
}
```

Each handle method that waits for a reply calls it with a closure that only builds the message variant, as in `self.ask(|reply| Msg::Shutdown { reply })`. The type parameter has no bound, the `impl` argument is only `FnOnce(oneshot::Sender<T>) -> Msg`, and the closure is allowed only as `ask`'s argument. A handle with a free-function helper (codex's `CodexServer` and `CodexThread` share one command sender) takes the sender as the first parameter instead of `&self`. Nothing else gets a type parameter outside 0013, or an `impl Trait` argument.

This revisits the `ask<T>`/`request<T>` part of the bootstrap review's §A11. §A11 measured only `crates/daemon/src/host.rs`, where the helper saved 9%. The agents rewrite adds two more handles, and across all three the saving is 16.6%.

## Before
`crates/agents/src/claude/session.rs:171-224` as rewritten. Each method builds and awaits its own `oneshot`, through private `send` and `ask_control` helpers. Codex's `server.rs:167-187` and `227-243` has the same shape with free `request`, `ask` and `send` functions. The host's three methods are §A11's before.

```rust
pub async fn send_user(&self, text: String) -> Result<(), Error> {
    let (reply, answer) = oneshot::channel();
    self.send(CommandMsg::SendUser { text, reply }).await?;
    answer.await.map_err(|_| Error::Exited)?
}

pub async fn shutdown(self) -> Result<ExitStatus, Error> {
    let (reply, answer) = oneshot::channel();
    self.send(CommandMsg::Shutdown { reply }).await?;
    answer.await.map_err(|_| Error::Exited)
}

async fn control(&self, call: ControlCall) -> Result<Value, Error> {
    tokio::time::timeout(CONTROL_TIMEOUT, self.ask_control(call))
        .await
        .map_err(|_| Error::Timeout(String::from(call.subtype())))?
}

async fn ask_control(&self, call: ControlCall) -> Result<Value, Error> {
    let (reply, answer) = oneshot::channel();
    self.send(CommandMsg::Control { call, reply }).await?;
    answer.await.map_err(|_| Error::Exited)?
}

async fn send(&self, msg: CommandMsg) -> Result<(), Error> {
    self.commands.send(msg).await.map_err(|_| Error::Exited)
}
```

## After
```rust
pub async fn send_user(&self, text: String) -> Result<(), Error> {
    self.ask(|reply| CommandMsg::SendUser { text, reply }).await?
}

pub async fn shutdown(self) -> Result<ExitStatus, Error> {
    self.ask(|reply| CommandMsg::Shutdown { reply }).await
}

async fn control(&self, call: ControlCall) -> Result<Value, Error> {
    tokio::time::timeout(CONTROL_TIMEOUT, self.ask(|reply| CommandMsg::Control { call, reply }))
        .await
        .map_err(|_| Error::Timeout(String::from(call.subtype())))??
}

async fn ask<T>(&self, build: impl FnOnce(oneshot::Sender<T>) -> CommandMsg) -> Result<T, Error> {
    let (reply, answer) = oneshot::channel();
    self.commands.send(build(reply)).await.map_err(|_| Error::Exited)?;
    answer.await.map_err(|_| Error::Exited)
}
```

The full before and after for all three handles are `docs/subset/measurements/0026/before.rs` and `after.rs`. The two agents rewrites were applied to a scratch worktree. They compile, pass `cargo clippy --all-targets -- -D warnings` and pass all 31 `agents` tests, including both replay suites, which drive every handle method against recorded CLI output. `subset check` then reports `generics.type_param` and `type.impl_trait.arg` on each `ask`, and `expr.closure` on the three codex call sites, which are free-function calls. It doesn't flag the closures passed to `self.ask`, because it can't tell that `ask` is our method (see the review's tool notes).

## Measurements
| | tokens | lines |
|---|---|---|
| before | 932 | 94 |
| after | 777 | 92 |
| change | -155 (-16.6%) | -2 |

By handle: claude `ClaudeSession` 408 to 332 (-18.6%), codex `CodexServer`/`CodexThread` 314 to 253 (-19.4%), daemon `Host` 210 to 192 (-8.6%, §A11's numbers).

Friction points removed: 2 (C4, Y1), plus §A11's host.rs site.

## Qualities
1. Named states: kept. The message is still a named variant, built in the caller.
2. Visible branching: kept. There is no branching in the helper. `control` and `request` end in `??`, two error exits in a row (timeout, then the actor's answer), each still a `?`.
3. Forward data flow: kept.
4. Errors are values: kept. The same errors pass through the same `map_err`s.
5. Local reasoning: kept. `ask` sits in the same `impl` as its callers and is five lines. The closure only builds a variant, and `T` is the type of that variant's `reply` field.
6. One way: helped. Today the crate has two shapes for the round trip (claude's methods with `send` and `ask_control`, codex's free `request`, `ask` and `send`), and each handle method repeats the `oneshot` steps. After, every round trip is one `ask` call.

## Owner decision
