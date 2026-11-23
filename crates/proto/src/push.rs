//! Web push: the subscriptions a host notifies, and the key it signs with.

use alloc::string::String;
use core::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushSubscription {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
}

impl fmt::Debug for PushSubscription {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str("PushSubscription")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum PushCommand {
    PublicKey,
    Register(PushSubscription),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum PushResult {
    Accepted,
    PublicKey(String),
    /// The host sends no push.
    Unavailable,
}
