---
construct: generics.type_param
status: proposed
task: bootstrap round (docs/subset/bootstrap-report.txt)
---

# Generic parameters with one serde bound

## Rule
A generic type parameter is allowed only on a codec function at the wire boundary, with exactly one bound, `T: Serialize` or `T: DeserializeOwned`, and no `where` clause. Today these are `proto::encode`, `proto::decode`, `SendHalf::write` and `RecvHalf::read`. Generic structs, enums and traits are not allowed, and neither are other bounds. 0009 covers generics that a trait impl's signature demands. The request helpers `ask<T>`/`request<T>` are rewritten per method (§A11).

## Before
Subset-only: the codec moves `serde_json::Value`, and every caller converts. From `crates/daemon/src/wire.rs`, `crates/proto/src/frame.rs` and three callers in `link.rs`/`server.rs`:

```rust
impl SendHalf {
    pub async fn write(&mut self, msg: serde_json::Value) -> Result<(), WireError> {
        let frame = proto::encode(msg).map_err(WireError::Frame)?;
        self.write_frame(frame).await
    }
}

impl RecvHalf {
    pub async fn read(&mut self) -> Result<Option<serde_json::Value>, WireError> {
        let body = self.read_body().await?;
        body.map(|body| proto::decode(&body)).transpose().map_err(WireError::Frame)
    }
}

pub fn encode(msg: serde_json::Value) -> Result<Vec<u8>, FrameError> {
    let body = serde_json::to_vec(&msg).map_err(FrameError::Json)?;
    frame(body)
}

pub fn decode(body: &[u8]) -> Result<serde_json::Value, FrameError> {
    serde_json::from_slice(body).map_err(FrameError::Json)
}

async fn hello(send: &mut SendHalf, recv: &mut RecvHalf, hello: Hello) -> Result<Option<HelloReply>, Lost> {
    send.write(serde_json::to_value(Open::Hello(hello)).map_err(Lost::Json)?).await?;
    match recv.read().await.map_err(Lost::Wire)? {
        Some(value) => serde_json::from_value::<HelloReply>(value).map(Some).map_err(Lost::Json),
        None => Ok(None),
    }
}

async fn rpc(send: &mut SendHalf, recv: &mut RecvHalf, command: Command) -> Result<Option<CommandResult>, SendError> {
    send.write(serde_json::to_value(Open::Command(command)).map_err(SendError::Json)?).await?;
    match recv.read().await.map_err(SendError::Wire)? {
        Some(value) => serde_json::from_value::<CommandResult>(value).map(Some).map_err(SendError::Json),
        None => Ok(None),
    }
}

async fn serve(send: &mut SendHalf, recv: &mut RecvHalf) -> Result<Option<Open>, StreamError> {
    send.write(serde_json::to_value(Event::default_for_test()).map_err(StreamError::Json)?).await?;
    match recv.read().await.map_err(StreamError::Wire)? {
        Some(value) => serde_json::from_value::<Open>(value).map(Some).map_err(StreamError::Json),
        None => Ok(None),
    }
}
```

## After
```rust
impl SendHalf {
    pub async fn write<T: Serialize>(&mut self, msg: &T) -> Result<(), WireError> {
        let frame = proto::encode(msg).map_err(WireError::Frame)?;
        self.write_frame(frame).await
    }
}

impl RecvHalf {
    pub async fn read<T: DeserializeOwned>(&mut self) -> Result<Option<T>, WireError> {
        let body = self.read_body().await?;
        body.map(|body| proto::decode(&body)).transpose().map_err(WireError::Frame)
    }
}

pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, FrameError> {
    let body = serde_json::to_vec(msg).map_err(FrameError::Json)?;
    frame(body)
}

pub fn decode<T: DeserializeOwned>(body: &[u8]) -> Result<T, FrameError> {
    serde_json::from_slice(body).map_err(FrameError::Json)
}

async fn hello(send: &mut SendHalf, recv: &mut RecvHalf, hello: Hello) -> Result<Option<HelloReply>, Lost> {
    send.write(&Open::Hello(hello)).await?;
    recv.read::<HelloReply>().await.map_err(Lost::Wire)
}

async fn rpc(send: &mut SendHalf, recv: &mut RecvHalf, command: Command) -> Result<Option<CommandResult>, SendError> {
    send.write(&Open::Command(command)).await?;
    recv.read::<CommandResult>().await.map_err(SendError::Wire)
}

async fn serve(send: &mut SendHalf, recv: &mut RecvHalf) -> Result<Option<Open>, StreamError> {
    send.write(&Event::default_for_test()).await?;
    recv.read::<Open>().await.map_err(StreamError::Wire)
}
```

## Measurements
| | tokens | lines |
|---|---|---|
| before | 513 | 40 |
| after | 387 | 31 |
| change | -126 (-25%) | -9 (-22%) |

This covers three of the 24 call sites. Each further caller adds about 40 tokens to the subset-only form, going by this sample.

Friction points removed: 16 `generics.type_param` sites. 7 are codec functions and fit the rule. 3 are the `ask<T>`/`request<T>` helpers and 4 are the parameters of the two `Result<T, E = Error>` aliases. Those are rewritten by hand (§A11; the helpers measured -9% over 3 methods, under the bar). 2 are serde trait-impl signatures under 0009.

## Qualities
1. Named states: kept, and helped. Callers read and write typed messages. The subset-only form passes untyped `Value`s around.
2. Visible branching: kept.
3. Forward data flow: kept.
4. Errors are values: kept.
5. Local reasoning: kept. One bound, no where clause: the signature says everything.
6. One way: kept. One codec, called the same way for every message type.

## Owner decision
