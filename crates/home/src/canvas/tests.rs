use super::{
    Block, Canvas, CanvasError, CanvasFile, CanvasName, ChangeKind, Choice,
    Options, OptionsError, StatechartError, Tag,
};

fn file() -> Result<CanvasFile, CanvasError> {
    Ok(CanvasFile {
        written: 1_795_000_000_000,
        by: String::from("devbox·claude"),
        canvas: Canvas::new(
            String::from("Why a store"),
            vec![Block::Prose {
                text: String::from("The core decides; the store **keeps**."),
            }],
        )?,
    })
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
fn a_canvas_file_reads_back_as_it_was_written() -> Result<(), CanvasError> {
    assert_eq!(CanvasFile::parse(&file()?.encode()), file());
    Ok(())
}

#[test]
fn a_canvas_is_read_from_its_json_with_each_block_named_by_its_kind()
-> Result<(), CanvasError> {
    let text = r#"{"title": "Why a store", "blocks": [{"prose": {"text": "The core decides; the store **keeps**."}}]}"#;
    assert_eq!(Canvas::parse(text), Ok(file()?.canvas));
    Ok(())
}

#[test]
fn json_that_isnt_a_canvas_or_a_canvas_file_is_refused_as_not_a_canvas() {
    let refused = [
        Canvas::parse(r#"{"blocks": []}"#).err(),
        CanvasFile::parse(r#"{"canvas": {"title": "t", "blocks": []}}"#).err(),
    ];
    for found in refused {
        assert!(
            found.as_ref().is_some_and(|error| {
                matches!(error, CanvasError::Json { .. })
                    && error.to_string().starts_with("not a canvas: ")
            }),
            "{found:?}"
        );
    }
}

#[test]
fn a_canvas_file_whose_canvas_breaks_a_drawing_rule_is_refused_as_a_canvas_is()
{
    let canvas = r#"{"title": "t", "blocks": [{"statechart": {"states": [{"id": "open", "label": "Open"}], "start": "open", "transitions": [{"from": "open", "to": "gone", "event": "go"}], "path": []}}]}"#;
    let file = format!(
        r#"{{"written": 5, "by": "devbox·claude", "canvas": {canvas}}}"#
    );
    let refused = Err(CanvasError::Statechart(StatechartError::UnknownTo {
        state: String::from("gone"),
    }));
    assert_eq!(
        (
            CanvasFile::parse(&file).map(drop),
            Canvas::parse(canvas).map(drop)
        ),
        (refused.clone(), refused)
    );
}

#[test]
fn a_canvas_built_with_a_block_that_breaks_a_drawing_rule_is_refused_as_decoding_it_is()
 {
    let built = Canvas::new(
        String::from("t"),
        vec![Block::Options(Options {
            question: String::from("Where?"),
            options: vec![Choice {
                label: String::from("Here"),
                gets: String::from("sync"),
                costs: String::from("size"),
            }],
            pick: 3,
            why: String::from("it syncs"),
        })],
    );
    assert_eq!(
        built,
        Err(CanvasError::Options(OptionsError::PastLast {
            pick: 3,
            last: 0
        }))
    );
}

const CROWDED: &str = r#"{"title": "t", "blocks": [{"diagram": {"nodes": [{"id": "a", "label": "A", "step": 1}, {"id": "a", "label": "B", "step": 1}], "edges": [], "groups": []}}]}"#;

#[test]
fn decoding_a_canvas_or_a_canvas_file_that_breaks_a_drawing_rule_refuses_it_by_any_path()
 {
    let said = "two nodes are called `a`: give each node its own id";
    let file = format!(
        r#"{{"written": 5, "by": "devbox·claude", "canvas": {CROWDED}}}"#
    );
    let decodes = [
        serde_json::from_str::<Canvas>(CROWDED).map(|_| ()),
        serde_json::from_str::<CanvasFile>(&file).map(|_| ()),
        serde_json::from_str::<serde_json::Value>(&file)
            .and_then(serde_json::from_value::<CanvasFile>)
            .map(|_| ()),
    ];
    for decoded in decodes {
        let shown = decoded.map_err(|error| error.to_string());
        assert!(
            shown.as_ref().is_err_and(|reason| reason.contains(said)),
            "{shown:?}"
        );
    }
    assert_eq!(
        CanvasFile::parse(&file).map_err(|error| error.to_string()),
        Err(String::from(said))
    );
}

#[test]
fn a_change_group_s_kind_and_a_change_s_tag_are_written_as_their_words()
-> Result<(), serde_json::Error> {
    let kinds: Vec<ChangeKind> =
        serde_json::from_str(r#"["core", "wiring", "mechanical"]"#)?;
    let tags: Vec<Tag> =
        serde_json::from_str(r#"["subtle", "breaking", "race", "perf"]"#)?;
    assert_eq!(
        (
            serde_json::to_string(&kinds)?,
            serde_json::to_string(&tags)?
        ),
        (
            String::from(r#"["core","wiring","mechanical"]"#),
            String::from(r#"["subtle","breaking","race","perf"]"#)
        )
    );
    assert_eq!(
        serde_json::from_str::<ChangeKind>(r#""tests""#)
            .map_err(|error| error.to_string()),
        Err(String::from(
            "unknown variant `tests`, expected one of `core`, `wiring`, `mechanical`"
        ))
    );
    Ok(())
}
