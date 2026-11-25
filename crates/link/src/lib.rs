#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod command_id;
pub mod frame;
pub mod log;
pub mod serve;
mod ticket;

pub use command_id::CommandId;
pub use ticket::{AuthToken, Ticket, WrongToken};

use alloc::string::String;

use agents::ClientName;
use home::{Home, HomeCommand, HomeResult};
use host::fleet::{DeviceHello, FleetCommand, FleetResult};
use host::log::{Epoch, Event, HostSeq};
use host::push::{PushCommand, PushResult};
use host::registry::{Catalog, InstanceCommand, InstanceResult, Instances};
use host::session::{SessionCommand, SessionResult};
use host::sign_in::{SignIn, SignInCommand, SignInResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const ALPN: &[u8] = b"conductor/47";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Open {
    Hello(Hello),
    Subscribe(Subscribe),
    Command { id: CommandId, body: Command },
    FleetHello(DeviceHello),
}

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
    pub catalog: Catalog,
    pub home: Option<Home>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub struct Subscribe {
    pub epoch: Epoch,
    pub after_seq: HostSeq,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Feed {
    Event(Event),
    SignIn(SignIn),
    Home(Home),
    Instances(Instances),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Command {
    Session(SessionCommand),
    Instance(InstanceCommand),
    SignIn(SignInCommand),
    Home(HomeCommand),
    Push(PushCommand),
    Fleet(FleetCommand),
}

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
    use host::session::{SessionId, SessionRefusal, SessionResult};

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

    #[test]
    fn a_session_id_is_written_on_the_link_as_in_its_branch_and_logs()
    -> Result<(), Box<dyn std::error::Error>> {
        let id = SessionId(u128::MAX);
        assert_eq!(id.to_string(), "7ZZZZZZZZZZZZZZZZZZZZZZZZZ");
        assert_eq!(serde_json::to_string(&id)?, format!("\"{id}\""));
        Ok(())
    }
}
