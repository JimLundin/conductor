//! The client link (architecture §5): the frames a host and the app send on
//! a connection, and the rules of a connection.
//!
//! The frames are built from the domains' commands, results, refusals and
//! feeds. It is the pure core: `no_std`, with the stream I/O in the `net`
//! crate and the connection run by `daemon`.
//!
//! Every bi-stream starts with one [`Open`] frame from the client. The first
//! stream of a connection must be [`Open::Hello`], answered by one
//! [`HelloReply`]. A [`Open::Command`] stream gets one [`CommandResult`] and
//! closes; both name the domain the command acts on, whose own enum the
//! link wraps. A [`Open::Subscribe`] stream then carries [`Feed`] frames, the
//! events and the sign-ins, until either side closes it.
//!
//! Frame bodies on this link are JSON ([`frame`]), which names each variant
//! and field: a renamed, added or removed one fails an older peer. So any
//! change to a type reachable from [`Open`], [`HelloReply`],
//! [`CommandResult`] or [`Feed`] bumps [`ALPN`], so that peer fails at the
//! handshake instead of mid-stream. Host stores are postcard, where a
//! variant is its index and a field its position, so there a reordered type
//! decodes as the wrong value with no error.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod frame;
pub mod log;
#[cfg(test)]
mod schema;
pub mod serve;

use alloc::string::String;

use proto::fleet::{DeviceHello, FleetCommand, FleetResult};
use proto::{
    AuthToken, Catalog, ClientName, CommandId, Epoch, Event, Home, HomeCommand,
    HomeResult, HostSeq, InstanceCommand, InstanceResult, Instances,
    PushCommand, PushResult, SessionCommand, SessionResult, SignIn,
    SignInCommand, SignInResult,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The ALPN of the client link. It changes with every change to the wire
/// types (see the crate docs), so an old client fails at the handshake
/// instead of mid-stream.
pub const ALPN: &[u8] = b"conductor/41";

/// The first frame on every client-opened bi-stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Open {
    Hello(Hello),
    Subscribe(Subscribe),
    Command { id: CommandId, body: Command },
    FleetHello(DeviceHello),
}

/// S3 auth placeholder: the ticket's token stands in for the `DeviceCert`
/// that §4 puts here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Hello {
    pub client: ClientName,
    pub token: AuthToken,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum HelloReply {
    Welcome(Welcome),
    Refused { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Welcome {
    pub host: String,
    pub epoch: Epoch,
    pub head_seq: HostSeq,
    pub instances: Instances,
    /// How each provider presents itself and the credentials it takes.
    /// Unlike `instances` it doesn't change while the host runs.
    pub catalog: Catalog,
    /// `None` on a host given no home. `Feed::Home` brings each change.
    pub home: Option<Home>,
}

/// Replays every stored event after `after_seq`, then streams live ones,
/// deltas included. A subscription from another epoch is refused by closing
/// the stream.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub struct Subscribe {
    pub epoch: Epoch,
    pub after_seq: HostSeq,
}

/// A logged event or a live update to the host's sign-ins, home or offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Feed {
    Event(Event),
    SignIn(SignIn),
    Home(Home),
    /// The current instance offers, replacing the previous snapshot.
    Instances(Instances),
}

/// A command, in the domain it acts on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Command {
    Session(SessionCommand),
    Instance(InstanceCommand),
    SignIn(SignInCommand),
    Home(HomeCommand),
    Push(PushCommand),
    Fleet(FleetCommand),
}

/// A command's answer, in the domain of the command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum CommandResult {
    Session(SessionResult),
    Instance(InstanceResult),
    SignIn(SignInResult),
    Home(HomeResult),
    Push(PushResult),
    Fleet(FleetResult),
}

#[cfg(test)]
mod tests {
    use proto::{SessionRefusal, SessionResult};

    use super::CommandResult;

    #[test]
    fn stored_receipts_use_the_current_format()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            postcard::from_bytes::<CommandResult>(&[0, 0])?,
            CommandResult::Session(SessionResult::Accepted)
        );
        assert_eq!(
            postcard::from_bytes::<CommandResult>(&[0, 2, 3])?,
            CommandResult::Session(SessionResult::Refused(
                SessionRefusal::SessionClosed
            ))
        );
        Ok(())
    }
}
