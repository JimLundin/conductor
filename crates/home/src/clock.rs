use core::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(transparent)]
pub struct UnixSecs(pub u64);

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(transparent)]
pub struct UnixMillis(pub u64);

const MILLIS_PER_SEC: u64 = 1000;

impl UnixSecs {
    #[must_use]
    pub const fn since_epoch(since: Duration) -> Self {
        Self(since.as_secs())
    }

    #[must_use]
    pub const fn millis(self) -> UnixMillis {
        UnixMillis(self.0.saturating_mul(MILLIS_PER_SEC))
    }

    #[must_use]
    pub const fn elapsed_since(self, earlier: Self) -> Duration {
        Duration::from_secs(self.0.saturating_sub(earlier.0))
    }
}

impl UnixMillis {
    #[must_use]
    pub fn since_epoch(since: Duration) -> Self {
        Self(u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
    }

    #[must_use]
    pub const fn secs(self) -> UnixSecs {
        UnixSecs(self.0 / MILLIS_PER_SEC)
    }

    #[must_use]
    pub const fn subsec_millis(self) -> u64 {
        self.0 % MILLIS_PER_SEC
    }
}

#[cfg(test)]
mod tests;
