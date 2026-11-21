//! Session handoffs between certified hosts in the same fleet.

use serde::{Deserialize, Serialize};

/// The identity shared by the move on the source and on the target.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct MoveId(pub u128);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Settling,
    CheckingTarget,
    SendingWorkspace,
    SendingHistory,
    Committing,
}
