use alloc::format;
use alloc::string::String;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CodeRef {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<(u32, u32)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}

impl CodeRef {
    pub(super) fn link(&self) -> String {
        let (shown, anchor) = match self.lines {
            None => (String::new(), String::new()),
            Some((first, last)) if first == last => {
                (format!(":{first}"), format!("#L{first}"))
            }
            Some((first, last)) => {
                (format!(":{first}-{last}"), format!("#L{first}-L{last}"))
            }
        };
        let commit = self
            .commit
            .as_ref()
            .map_or_else(String::new, |commit| format!(" at {commit}"));
        format!("[{path}{shown}{commit}]({path}{anchor})", path = self.path)
    }
}
