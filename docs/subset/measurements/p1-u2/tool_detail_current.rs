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

/// The `input` field worth showing for each tool that has one, and its shape.
fn tool_field(tool: &str) -> Option<(&str, Shape)> {
    match tool {
        "Bash" => Some(("command", Shape::Command)),
        "Edit" | "Write" | "MultiEdit" | "Read" => Some(("file_path", Shape::Path)),
        "Grep" | "Glob" => Some(("pattern", Shape::Text)),
        "WebFetch" => Some(("url", Shape::Text)),
        _ => None,
    }
}

pub(crate) fn tool_detail(tool: &str, input: &Value) -> ToolDetail {
    let shown = tool_field(tool)
        .and_then(|(field, shape)| Some((shape, input.get(field)?.as_str()?.to_owned())));
    match shown {
        Some((Shape::Command, text)) => ToolDetail::Command(text),
        Some((Shape::Path, text)) => ToolDetail::Path(text),
        Some((Shape::Text, text)) => ToolDetail::Text(text),
        None => ToolDetail::Text(input.to_string()),
    }
}

