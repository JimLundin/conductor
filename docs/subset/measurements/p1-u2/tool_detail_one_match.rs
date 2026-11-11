use serde_json::Value;

/// What a tool call is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolDetail {
    Command(String),
    Path(String),
    Text(String),
}

/// The `input` field worth showing for each tool that has one, in its shape.
pub(crate) fn tool_detail(tool: &str, input: &Value) -> ToolDetail {
    let shown = match tool {
        "Bash" => field(input, "command").map(ToolDetail::Command),
        "Edit" | "Write" | "MultiEdit" | "Read" => field(input, "file_path").map(ToolDetail::Path),
        "Grep" | "Glob" => field(input, "pattern").map(ToolDetail::Text),
        "WebFetch" => field(input, "url").map(ToolDetail::Text),
        _ => None,
    };
    shown.unwrap_or_else(|| ToolDetail::Text(input.to_string()))
}

fn field(input: &Value, name: &str) -> Option<String> {
    input.get(name).and_then(Value::as_str).map(str::to_owned)
}
