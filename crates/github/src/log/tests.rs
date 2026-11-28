use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use home::UnixMillis;
use home::canvas::{Block, Canvas, CanvasName};
use home::issue::{Entry, LogEvent, Note};

use super::{Log, Posted, comment, read};

const MARKER: &str = "conductor-operation 0707/11";

fn posted(id: &str, body: &str) -> Posted {
    Posted {
        id: String::from(id),
        author: String::from("reader"),
        created: UnixMillis(5000),
        body: String::from(body),
    }
}

fn moved() -> Entry {
    Entry {
        at: Some(String::from("2026-11-28T10:00:00Z")),
        by: Some(String::from("devbox·claude")),
        event: LogEvent::Stage {
            from: String::from("spec"),
            to: String::from("slice"),
        },
        text: String::from("signed off\nby the owner"),
    }
}

fn note(id: &str, text: &str) -> Note {
    Note {
        id: String::from(id),
        author: String::from("reader"),
        created: UnixMillis(5000),
        text: String::from(text),
    }
}

#[test]
fn a_log_comment_reads_back_as_the_entry_it_was_written_from() {
    let body = comment(&moved(), MARKER);
    assert_eq!(
        body,
        "<!-- conductor-log\nconductor-operation 0707/11\n-->\n2026-11-28T10:00:00Z devbox·claude stage spec→slice: signed off\nby the owner\n"
    );
    assert_eq!(
        read(Vec::from([posted("1", &body.replace('\n', "\r\n"))])),
        Log {
            entries: Vec::from([moved()]),
            notes: Vec::new(),
        }
    );
}

#[test]
fn a_person_s_comment_is_a_note_even_when_it_reads_like_a_move_and_a_canvas_is_neither()
-> Result<(), String> {
    let said = "2026-11-28T10:00:00Z owner stage spec→slice: signed off";
    let canvas = crate::canvas::comment(
        &CanvasName::parse("design").map_err(|error| error.to_string())?,
        "devbox·claude",
        MARKER,
        &Canvas::new(
            String::from("Why"),
            Vec::from([Block::Prose {
                text: String::from("Because."),
            }]),
        )
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let quoted = format!("> {}", comment(&moved(), MARKER));
    assert_eq!(
        read(Vec::from([
            posted("1", said),
            posted("2", &canvas),
            posted("3", &quoted)
        ])),
        Log {
            entries: Vec::new(),
            notes: Vec::from([note("1", said), note("3", &quoted)]),
        }
    );
    Ok(())
}
