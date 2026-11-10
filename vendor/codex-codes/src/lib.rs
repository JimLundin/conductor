//! A typed Rust interface for the [OpenAI Codex CLI](https://github.com/openai/codex) protocol.
//!
//! Conductor's vendored copy: serde types for the `codex app-server` JSON-RPC
//! protocol plus a builder for its command line. It has no client. A caller
//! owns the child's stdio, writes [`JsonRpcRequest`]s and parses each stdout
//! line with [`JsonRpcMessage`] and [`ServerMessage`]. See
//! `docs/codex-codes-gaps.md` in the Conductor repository for why the
//! upstream clients were removed.
//!
//! # Modules
//!
//! - [`protocol`]: app-server v2 request params, response types and
//!   notification bodies, generated from the upstream JSON Schema.
//! - [`messages`]: typed dispatch for notifications and server-to-client
//!   requests, with `Unknown` variants for methods this crate does not model.
//! - [`jsonrpc`]: JSON-RPC envelopes. The app-server omits `"jsonrpc": "2.0"`.
//! - [`cli`] (feature `cli`): builder for `codex app-server --listen stdio://`.
//!
//! # Feature Flags
//!
//! | Feature | Description | WASM-compatible |
//! |---------|-------------|-----------------|
//! | `types` | Message types and protocol structs only | Yes |
//! | `cli` | `AppServerBuilder` and `auth_local` | No |
//!
//! # Parsing Raw Protocol Messages
//!
//! ```
//! use codex_codes::{ThreadEvent, ThreadItem, JsonRpcMessage};
//!
//! // Parse exec-format JSONL events
//! let json = r#"{"type":"thread.started","thread_id":"th_abc"}"#;
//! let event: ThreadEvent = serde_json::from_str(json).unwrap();
//!
//! // Parse app-server JSON-RPC messages
//! let rpc = r#"{"id":1,"result":{"threadId":"th_abc"}}"#;
//! let msg: JsonRpcMessage = serde_json::from_str(rpc).unwrap();
//! ```

pub mod io;

pub mod error;
pub mod events;
pub mod jsonrpc;
pub mod messages;
pub mod models;
pub mod protocol;
pub mod protocol_generated;

#[cfg(feature = "cli")]
pub mod auth_local;
#[cfg(feature = "cli")]
pub mod cli;

#[cfg(feature = "cli")]
mod process;

// Exec-level event types (JSONL protocol)
pub use io::events::{
    ItemCompletedEvent, ItemStartedEvent, ItemUpdatedEvent, ThreadError, ThreadErrorEvent,
    ThreadEvent, ThreadStartedEvent, TurnCompletedEvent, TurnFailedEvent, TurnStartedEvent, Usage,
};

// Error types (always available)
pub use error::{Error, ParseError, Result};

// JSON-RPC types (always available)
pub use jsonrpc::{
    JsonRpcError, JsonRpcErrorData, JsonRpcMessage, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse, RequestId,
};

// App-server protocol types — generated from the upstream JSON Schema and
// re-exported through `protocol::*`.
pub use protocol::*;

// Typed message dispatch (notifications + server-to-client requests)
pub use messages::{Notification, ServerMessage, ServerRequest};

// Model catalog convenience enum
pub use models::CodexModel;

#[cfg(feature = "cli")]
pub use cli::AppServerBuilder;
