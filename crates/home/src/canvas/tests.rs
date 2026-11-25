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
