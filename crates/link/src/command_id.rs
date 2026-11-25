//! A command's id on the client link, which the app mints so a retried
//! command answers from its receipt.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A client-generated, fleet-unique random UUID for idempotent retries.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    JsonSchema,
)]
pub struct CommandId(pub Uuid);
