use alloc::{string::String, vec::Vec};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub path: String,
    pub roots: Vec<String>,
    pub remote: Option<String>,
}

#[must_use]
pub fn observed(
    path: String,
    top: Option<&str>,
    roots: Vec<String>,
    remote: Option<String>,
) -> Option<Probe> {
    (top == Some(path.as_str()) && !roots.is_empty()).then_some(Probe {
        path,
        roots,
        remote,
    })
}
