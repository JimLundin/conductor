use super::comments::{
    CREATED, LISTED, Writer, comment, human, lost, posted, sent,
};
use super::exchanges;
use super::tracking::{issue, lookup, reply, tracker};
use home::flow::command::{Change, Mutation};
use home::issue::body::Body;
use home::issue::{Entry, Issue, LogEvent, Note, Observation};
use home::operations::{Handle, Progress, Step};
use home::{IssueId, ProjectId, UnixMillis};
use project_runtime::{IssueTracker as _, WriteError};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const LIST: &str = "/repos/owner/work/issues?state=all&sort=created&direction=asc&per_page=100&page=1";
const OPEN: &str = "/repos/owner/work/issues";
const ISSUE: &str = "/repos/owner/work/issues/146";

fn handle(id: u128) -> Handle {
    Handle {
        owner: [7; 32],
        id: home::issue::EditId(id),
    }
}

fn opened() -> Entry {
    Entry {
        at: Some(String::from("2026-11-28T10:00:00Z")),
        by: Some(String::from("devbox·claude")),
        event: LogEvent::Note,
        text: String::from("opened."),
    }
}

fn change() -> anyhow::Result<Change> {
    let issue = Issue {
        id: home::tracker::Issuer::MARKDOWN
            .read("0v2abcde")
            .ok_or_else(|| anyhow::anyhow!("id"))?,
        project: ProjectId::new(1),
        created: Some(UnixMillis(1000)),
        title: String::from("Opened by home"),
        kind: String::from("task"),
        status: String::from("open"),
        stage: None,
        fields: BTreeMap::new(),
        links: BTreeMap::new(),
        body: String::new(),
        sections: Vec::new(),
        checklist: Vec::new(),
        log: Vec::new(),
        notes: Vec::new(),
        branch: String::new(),
        canvases: Vec::new(),
    };
    Ok(Change {
        observation: Observation::mapped(
            issue,
            Body::new(&[String::from("What")], &opened()),
        ),
        created: true,
        mutations: Vec::from([Mutation::Note(opened())]),
    })
}

fn made(handle: Handle) -> anyhow::Result<Value> {
    let create = tracker("http://127.0.0.1:9")?
        .opening(&change()?, handle)
        .map_err(anyhow::Error::msg)?;
    let mut made = issue(147, 9147);
    made["title"] = json!(create.title);
    made["body"] = json!(create.body);
    Ok(made)
}

fn created(bodies: &[hyper::body::Bytes]) -> Vec<Value> {
    bodies
        .iter()
        .filter(|body| !body.is_empty())
        .filter_map(|body| serde_json::from_slice::<Value>(body).ok())
        .collect()
}

async fn open(writer: &Writer, fresh: Handle) -> Result<IssueId, WriteError> {
    writer
        .registry
        .project(&writer.backends)
        .open_issue(
            &writer.operations,
            fresh,
            &change()
                .map_err(|error| WriteError::Unsupported(error.to_string()))?,
        )
        .await
}

#[tokio::test]
async fn opening_an_issue_sends_one_create_holding_its_marker_and_confirms_it_by_listing_issues()
-> anyhow::Result<()> {
    let handle = handle(21);
    let (base, served) = exchanges(vec![
        reply(LIST, 200, &json!([issue(146, 9146)])),
        sent("POST", OPEN, &made(handle)?),
        reply(LIST, 200, &json!([issue(146, 9146), made(handle)?])),
    ])
    .await?;
    let writer = Writer::new(&base)?;
    assert_eq!(open(&writer, handle).await?.to_string(), "147");
    assert_eq!(writer.progress(handle)?, Progress::Completed);
    let sent = created(&served.await??);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["title"], "Opened by home");
    assert_eq!(sent[0]["labels"], json!(["work/kind/task"]));
    let body = sent[0]["body"].as_str().unwrap_or_default();
    assert!(body.starts_with("## What\n\n```project-data\n"), "{body}");
    assert!(
        body.contains(&format!(
            "operation = \"{}/{}\"",
            "07".repeat(32),
            home::issue::EditId(21)
        )),
        "{body}"
    );
    Ok(())
}

#[tokio::test]
async fn a_lost_create_reply_retried_finds_its_marker_and_creates_nothing_more()
-> anyhow::Result<()> {
    let first = handle(22);
    let (base, served) = exchanges(vec![
        reply(LIST, 200, &json!([])),
        lost("POST", OPEN),
        reply(LIST, 200, &json!([made(first)?])),
    ])
    .await?;
    let writer = Writer::new(&base)?;
    assert!(matches!(
        open(&writer, first).await,
        Err(WriteError::Unknown(unknown)) if unknown == first
    ));
    assert_eq!(open(&writer, handle(23)).await?.to_string(), "147");
    assert_eq!(writer.progress(first)?, Progress::Completed);
    assert_eq!(writer.operations.status(handle(23))?, None);
    assert_eq!(created(&served.await??).len(), 1);
    Ok(())
}

#[tokio::test]
async fn an_empty_scan_after_a_lost_create_reply_sends_nothing_again()
-> anyhow::Result<()> {
    let first = handle(24);
    let (base, served) = exchanges(vec![
        reply(LIST, 200, &json!([])),
        lost("POST", OPEN),
        reply(LIST, 200, &json!([])),
    ])
    .await?;
    let writer = Writer::new(&base)?;
    assert!(open(&writer, first).await.is_err());
    assert!(matches!(
        open(&writer, handle(25)).await,
        Err(WriteError::Unknown(unknown)) if unknown == first
    ));
    assert_eq!(
        writer.progress(first)?,
        Progress::AwaitingEvidence(Step::CreateIssue)
    );
    assert_eq!(created(&served.await??).len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_note_is_one_marked_comment_and_reading_the_issue_lists_it_in_its_log()
-> anyhow::Result<()> {
    let handle = handle(26);
    let tracker = tracker("http://127.0.0.1:9")?;
    let entry = Entry {
        text: String::from("worker started"),
        ..opened()
    };
    let body = tracker.logged(&entry, handle);
    let note = comment(2, &body, "2026-11-02T10:00:00Z");
    let (base, served) = exchanges(vec![
        reply(LISTED, 200, &json!([human()])),
        reply(LISTED, 200, &json!([human()])),
        sent("POST", CREATED, &note),
        reply(LISTED, 200, &json!([human(), note.clone()])),
        reply(ISSUE, 200, &issue(146, 9146)),
        reply(LISTED, 200, &json!([human(), note])),
    ])
    .await?;
    let writer = Writer::new(&base)?;
    let project = writer.registry.project(&writer.backends);
    let posted_note = project
        .append_note(&writer.operations, handle, lookup("146")?, &entry)
        .await?;
    assert_eq!(posted_note.id, "2");
    let read = project.read_issue(lookup("146")?).await?;
    let (issue, _) = read.editable().map_err(anyhow::Error::msg)?;
    assert_eq!(issue.log, [entry]);
    assert_eq!(
        issue.notes,
        [Note {
            id: String::from("1"),
            author: String::from("reader"),
            created: UnixMillis(1_793_527_200_000),
            text: String::from("A person's note"),
        }]
    );
    assert_eq!(read.completeness.notes, home::issue::Coverage::Complete);
    assert_eq!(posted(&served.await??), [body]);
    Ok(())
}
