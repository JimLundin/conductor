use super::{Block, Canvas, CanvasError, CanvasFile, CanvasName};

fn file() -> CanvasFile {
    CanvasFile {
        written: 1_795_000_000_000,
        by: String::from("devbox·claude"),
        canvas: Canvas {
            title: String::from("Why a store"),
            blocks: vec![Block::Prose {
                text: String::from("The core decides; the store **keeps**."),
            }],
        },
    }
}

#[test]
fn a_canvas_name_is_lowercase_words_joined_by_dashes() {
    for name in ["design", "design-tree", "step-2"] {
        assert_eq!(
            CanvasName::parse(name).map(|parsed| parsed.to_string()),
            Ok(String::from(name))
        );
    }
}

#[test]
fn a_name_that_isnt_lowercase_words_joined_by_dashes_is_refused_saying_how_to_name_it()
 {
    for name in [
        "Design Tree",
        "",
        "design--tree",
        "-design",
        "design-",
        "a/b",
        "../x",
        "design_tree",
    ] {
        let refused =
            CanvasName::parse(name).map_err(|error| error.to_string());
        assert_eq!(
            refused,
            Err(format!(
                "`{name}` isn't a canvas name: name it with lowercase words joined by `-`, such as `design-tree`"
            ))
        );
    }
}

#[test]
fn a_canvas_file_reads_back_as_it_was_written() {
    assert_eq!(CanvasFile::parse(&file().encode()), Ok(file()));
}

#[test]
fn a_canvas_is_read_from_its_json_with_each_block_named_by_its_kind() {
    let text = r#"{"title": "Why a store", "blocks": [{"prose": {"text": "The core decides; the store **keeps**."}}]}"#;
    assert_eq!(Canvas::parse(text), Ok(file().canvas));
}

#[test]
fn json_that_isnt_a_canvas_is_refused_naming_what_is_wrong() {
    let cases = [
        (r#"{"blocks": []}"#, "missing field `title`"),
        (
            r#"{"title": "t", "blocks": [{"chart": {}}]}"#,
            "unknown variant `chart`",
        ),
        (
            r#"{"title": "t", "blocks": [], "colour": "red"}"#,
            "unknown field `colour`",
        ),
        (
            r#"{"title": "t", "blocks": [{"prose": {"text": "x", "size": 3}}]}"#,
            "unknown field `size`",
        ),
        (r#""a canvas""#, "expected struct Canvas"),
    ];
    for (text, said) in cases {
        let refused = Canvas::parse(text);
        assert!(
            matches!(&refused, Err(CanvasError::Json { reason }) if reason.contains(said)),
            "{text}: {refused:?}"
        );
        assert!(
            refused
                .map_err(|error| error.to_string())
                .is_err_and(|shown| shown.starts_with("not a canvas: ")),
            "{text}"
        );
    }
}

#[test]
fn a_canvas_file_with_no_stamp_is_refused() {
    let text = r#"{"canvas": {"title": "t", "blocks": []}}"#;
    assert!(
        matches!(CanvasFile::parse(text), Err(CanvasError::Json { reason }) if reason.contains("missing field `written`"))
    );
}

#[test]
fn a_canvas_file_whose_canvas_breaks_a_drawing_rule_is_refused_as_a_canvas_is()
{
    let canvas = r#"{"title": "t", "blocks": [{"statechart": {"states": [{"id": "open", "label": "Open"}], "start": "open", "transitions": [{"from": "open", "to": "gone", "event": "go"}], "path": []}}]}"#;
    let file = format!(
        r#"{{"written": 5, "by": "devbox·claude", "canvas": {canvas}}}"#
    );
    let refused = CanvasFile::parse(&file);
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(refused.err(), Canvas::parse(canvas).err());
}

const CROWDED: &str = r#"{"title": "t", "blocks": [{"diagram": {"nodes": [{"id": "a", "label": "A", "step": 1}, {"id": "a", "label": "B", "step": 1}], "edges": [], "groups": []}}]}"#;

#[test]
fn decoding_a_canvas_or_a_canvas_file_that_breaks_a_drawing_rule_refuses_it_by_any_path()
 {
    let said = "two nodes are called `a`: give each node its own id";
    let file = format!(
        r#"{{"written": 5, "by": "devbox·claude", "canvas": {CROWDED}}}"#
    );
    let decoded = serde_json::from_str::<Canvas>(CROWDED)
        .map_err(|error| error.to_string());
    assert!(
        decoded.as_ref().is_err_and(|shown| shown.contains(said)),
        "{decoded:?}"
    );
    let decoded = serde_json::from_str::<CanvasFile>(&file)
        .map_err(|error| error.to_string());
    assert!(
        decoded.as_ref().is_err_and(|shown| shown.contains(said)),
        "{decoded:?}"
    );
    let value = serde_json::from_str::<serde_json::Value>(&file)
        .map_err(|error| error.to_string());
    let decoded = value.and_then(|value| {
        serde_json::from_value::<CanvasFile>(value)
            .map_err(|error| error.to_string())
    });
    assert!(
        decoded.as_ref().is_err_and(|shown| shown.contains(said)),
        "{decoded:?}"
    );
    assert_eq!(
        CanvasFile::parse(&file).map_err(|error| error.to_string()),
        Err(String::from(said))
    );
}
