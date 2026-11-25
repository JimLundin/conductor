#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Any,
    Disabled,
    Enabled,
    Exact(&'static str),
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attribute<'a> {
    Set,
    Unset,
    Value(&'a str),
    Unspecified,
    NonUtf8,
}

pub const ATTRIBUTES: [&str; 9] = [
    "filter",
    "working-tree-encoding",
    "ident",
    "merge",
    "diff",
    "text",
    "eol",
    "crlf",
    "binary",
];

#[must_use]
pub fn accepts(name: &str, attribute: Attribute<'_>) -> bool {
    use Attribute::{Set, Unset, Unspecified, Value};
    matches!(
        (name, attribute),
        (_, Unspecified | Unset)
            | ("text" | "diff" | "merge" | "binary", Set)
            | ("text", Value("auto"))
            | ("eol", Value("lf"))
            | ("merge", Value("binary" | "text"))
    )
}

#[must_use]
pub fn configuration(name: &str) -> Setting {
    match name {
        "core.ignorecase"
        | "core.autocrlf"
        | "core.safecrlf"
        | "core.sparsecheckout"
        | "core.splitindex"
        | "core.sparsecheckoutcone"
        | "index.sparse"
        | "core.precomposeunicode"
        | "extensions.worktreeconfig" => Setting::Disabled,
        "core.filemode" | "core.symlinks" => Setting::Enabled,
        "core.eol" => Setting::Exact("lf"),
        "extensions.objectformat" => Setting::Exact("sha1"),
        "extensions.refstorage" => Setting::Exact("files"),
        _ if name.starts_with("extensions.") || name.starts_with("merge.") => {
            Setting::Unsupported
        }
        _ if name.starts_with("remote.") && name.ends_with(".promisor") => {
            Setting::Disabled
        }
        _ if name.starts_with("remote.")
            && name.ends_with(".partialclonefilter") =>
        {
            Setting::Unsupported
        }
        _ => Setting::Any,
    }
}

#[cfg(test)]
mod tests;
