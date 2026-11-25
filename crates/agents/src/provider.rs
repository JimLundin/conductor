use alloc::borrow::Cow;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(transparent)]
pub struct Provider(Cow<'static, str>);

impl Provider {
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        Self(Cow::Borrowed(name))
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Display for Provider {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_provider_travels_as_the_name_its_plugin_declares()
    -> Result<(), Box<dyn std::error::Error>> {
        let gemini = super::Provider::new("gemini");
        assert_eq!(serde_json::to_string(&gemini)?, "\"gemini\"");
        assert_eq!(
            serde_json::from_str::<super::Provider>("\"gemini\"")?,
            gemini
        );
        assert_eq!(
            postcard::from_bytes::<super::Provider>(&postcard::to_allocvec(
                &gemini
            )?)?,
            gemini
        );
        assert_eq!(gemini.name(), "gemini");
        Ok(())
    }
}
