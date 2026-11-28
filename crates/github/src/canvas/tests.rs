use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;

use home::canvas::{Block, Canvas, CanvasError, CanvasName, StrayReason};

use super::{LIMIT, Read, TooLong, comment, read};

const MARKER: &str = "conductor-operation 0707/11";

fn canvas(text: &str) -> Result<Canvas, CanvasError> {
    Canvas::new(
        String::from("Why a store"),
        vec![Block::Prose {
            text: String::from(text),
        }],
    )
}

fn name(text: &str) -> Result<CanvasName, String> {
    CanvasName::parse(text).map_err(|error| error.to_string())
}

fn written(by: &str) -> Result<String, String> {
    comment(
        &name("design")?,
        by,
        MARKER,
        &canvas("The core decides.").map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

fn kept(by: &str) -> Result<Option<Read>, String> {
    Ok(Some(Read::Canvas {
        name: name("design")?,
        by: String::from(by),
        canvas: canvas("The core decides."),
    }))
}

#[test]
fn a_canvas_comment_opens_with_its_marker_then_the_markdown_then_the_data()
-> Result<(), String> {
    assert_eq!(
        written("devbox·claude")?,
        "<!-- conductor-canvas\n\
         name: design\n\
         by: \"devbox·claude\"\n\
         conductor-operation 0707/11\n\
         -->\n\
         _Conductor keeps this canvas here, and `home canvas put` rewrites this comment on every put, so an edit to it is lost._\n\
         \n\
         # Why a store\n\
         \n\
         The core decides.\n\
         \n\
         <details>\n\
         <summary>Canvas data</summary>\n\
         \n\
         ```json\n\
         {\n  \"title\": \"Why a store\",\n  \"blocks\": [\n    {\n      \"prose\": {\n        \"text\": \"The core decides.\"\n      }\n    }\n  ]\n}\n\
         ```\n\
         \n\
         </details>\n"
    );
    Ok(())
}

#[test]
fn a_canvas_comment_reads_back_as_its_name_signer_and_canvas()
-> Result<(), String> {
    let by = "host --> <b>·cli\n";
    assert_eq!(read(&written(by)?), kept(by)?);
    Ok(())
}

#[test]
fn an_edit_to_the_markdown_on_github_leaves_the_canvas_as_it_was()
-> Result<(), String> {
    let edited = written("devbox·claude")?
        .replace(
            "The core decides.\n\n<details>",
            "Edited by hand.\n\n<details>",
        )
        .replace('\n', "\r\n");
    assert_eq!(read(&edited), kept("devbox·claude")?);
    Ok(())
}

#[test]
fn a_comment_without_the_marker_is_no_canvas() {
    assert_eq!(read("A person's note"), None);
}

#[test]
fn a_marked_comment_whose_name_or_data_doesnt_parse_is_a_stray()
-> Result<(), String> {
    let good = written("devbox·claude")?;
    let data = good.find("```json").ok_or("no data")?;
    assert_eq!(
        [
            read(&good.replace("name: design", "name: Design Tree")),
            read(&good.replace("name: design\n", "")),
            read(&good[..data]),
            read(&good.replace("\"title\"", "title")),
            read(&good.replace("\n-->\n", "\n")),
        ],
        [
            Some(Read::Stray(StrayReason::Unnamed)),
            Some(Read::Stray(StrayReason::Unnamed)),
            Some(Read::Stray(StrayReason::Unreadable)),
            Some(Read::Stray(StrayReason::Unreadable)),
            Some(Read::Stray(StrayReason::Unreadable)),
        ]
    );
    Ok(())
}

#[test]
fn marked_data_that_isnt_a_canvas_is_a_broken_canvas_of_its_name()
-> Result<(), String> {
    let good = written("devbox·claude")?;
    let (head, _) = good.split_once("```json\n").ok_or("no data")?;
    let broken = format!("{head}```json\n{{}}\n```\n\n</details>\n");
    assert!(matches!(
        read(&broken),
        Some(Read::Canvas { name: named, canvas: Err(CanvasError::Json { .. }), .. })
            if named == name("design")?
    ));
    Ok(())
}

#[test]
fn a_canvas_whose_comment_passes_github_s_limit_fails_with_its_size()
-> Result<(), String> {
    let long = canvas(&"word ".repeat(LIMIT / 4)).map_err(|e| e.to_string())?;
    let refused = comment(&name("design")?, "devbox·claude", MARKER, &long);
    let size = match &refused {
        Err(TooLong { size }) if *size > LIMIT => *size,
        other => return Err(format!("{other:?}")),
    };
    assert_eq!(
        refused.map_err(|error| error.to_string()),
        Err(format!(
            "its comment would be {size} characters, over GitHub's limit of 65536: split the canvas"
        ))
    );
    Ok(())
}
