use serde_json::Value;

/// What a tool call is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolDetail {
    Command(String),
    Path(String),
    Text(String),
}

#[derive(Debug, Clone, Copy)]
enum Shape {
    Command,
    Path,
    Text,
}

struct ToolField {
    tool: &'static str,
    field: &'static str,
    shape: Shape,
}

/// One row per tool whose input has a field worth showing.
const TOOL_FIELDS: &[ToolField] = &[
    ToolField { tool: "Bash", field: "command", shape: Shape::Command },
    ToolField { tool: "Edit", field: "file_path", shape: Shape::Path },
    ToolField { tool: "Write", field: "file_path", shape: Shape::Path },
    ToolField { tool: "MultiEdit", field: "file_path", shape: Shape::Path },
    ToolField { tool: "Read", field: "file_path", shape: Shape::Path },
    ToolField { tool: "Grep", field: "pattern", shape: Shape::Text },
    ToolField { tool: "Glob", field: "pattern", shape: Shape::Text },
    ToolField { tool: "WebFetch", field: "url", shape: Shape::Text },
];

pub(crate) fn tool_detail(tool: &str, input: &Value) -> ToolDetail {
    let shown = TOOL_FIELDS
        .iter()
        .find(|row| row.tool == tool)
        .and_then(|row| Some((row.shape, input.get(row.field)?.as_str()?.to_owned())));
    match shown {
        Some((Shape::Command, text)) => ToolDetail::Command(text),
        Some((Shape::Path, text)) => ToolDetail::Path(text),
        Some((Shape::Text, text)) => ToolDetail::Text(text),
        None => ToolDetail::Text(input.to_string()),
    }
}
