use super::tracking::{issue, lookup, reply, tracker};
use super::{Exchange, exchanges};
use home::backend::{Backends, Capability, CredentialState};
use home::flow::command::Mutation;
use home::issue::{Entry, LogEvent, NativeIssue, Part};
use home::operations::{Handle, Refusal};
use project_runtime::operations::{Error, Operations};
use project_runtime::{IssueEdit, Registration, Registry, WriteError};
use serde_json::{Value, json};
use std::sync::Arc;

const ISSUE: &str = "/repos/owner/work/issues/146";
const LABELS: &str = "/repos/owner/work/issues/146/labels";
const SHAPE: &str = "/repos/owner/work/issues/146/labels/work%2Fstage%2Fshape";
const FOOTER: &str =
    "```project-data\nschema = 1\n\n[fields]\nbranch = \"kept\"\n```\n";

fn label(name: &str) -> Value {
    json!({
        "id": 1, "node_id": "LA_1", "url": "{api}/labels/1",
        "name": name, "color": "ededed", "default": false, "description": null
    })
}

fn observed(labels: &[&str], body: &str) -> Value {
    let mut value = issue(146, 9146);
    value["labels"] = labels.iter().copied().map(label).collect();
    value["body"] = json!(body);
    value
}

fn native(labels: &[&str], body: &str) -> NativeIssue {
    NativeIssue {
        title: String::from("Native issue"),
        body: String::from(body),
        labels: labels.iter().copied().map(String::from).collect(),
        open: true,
        reason: None,
    }
}

fn sent(method: &'static str, path: &'static str, body: &Value) -> Exchange {
    Exchange {
        method,
        ..reply(path, 200, body)
    }
}

fn lost(method: &'static str, path: &'static str) -> Exchange {
    Exchange {
        lost: true,
        ..sent(method, path, &json!([]))
    }
}

fn move_to(stage: &str) -> Vec<Mutation> {
    Vec::from([
        Mutation::Move {
            part: Part::Stage,
            to: String::from(stage),
        },
        Mutation::Note(Entry {
            at: None,
            by: None,
            event: LogEvent::Note,
            text: String::from("moved"),
        }),
    ])
}

struct Editor {
    _dir: tempfile::TempDir,
    operations: Operations,
    registry: Registry,
    backends: Backends,
}

impl Editor {
    fn new(base: &str) -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        let operations = Operations::adopt(dir.path(), || Ok([7; 32]))?;
        let selection = super::selection("owner/work");
        let mut registry = Registry::default();
        registry
            .register_tracker(
                Registration {
                    selection: selection.clone(),
                    capabilities: vec![
                        Capability::ReadIssues,
                        Capability::WriteIssues,
                    ],
                    credential: CredentialState::Available,
                },
                Arc::new(tracker(base)?),
            )
            .map_err(|refusal| anyhow::anyhow!("{refusal:?}"))?;
        Ok(Self {
            _dir: dir,
            operations,
            registry,
            backends: Backends {
                tracker: Some(selection),
                ..Backends::default()
            },
        })
    }

    async fn edit(
        &self,
        handle: Handle,
        seen: &NativeIssue,
        mutations: Vec<Mutation>,
    ) -> Result<NativeIssue, WriteError> {
        let issue = lookup("146").map_err(|_| {
            WriteError::Read(home::issue::ReadFailure::InvalidResponse)
        })?;
        self.registry
            .project(&self.backends)
            .edit_issue(
                &self.operations,
                handle,
                IssueEdit {
                    issue,
                    seen: seen.revision(),
                    mutations,
                },
            )
            .await
    }
}

fn bodies(received: &[hyper::body::Bytes]) -> Vec<Value> {
    received
        .iter()
        .filter(|body| !body.is_empty())
        .filter_map(|body| serde_json::from_slice(body).ok())
        .collect()
}

#[tokio::test]
async fn a_stage_move_swaps_the_stage_label_and_sends_nothing_else()
-> anyhow::Result<()> {
    let before = ["work/stage/shape"];
    let (base, served) = exchanges(vec![
        reply(ISSUE, 200, &observed(&before, "Words")),
        sent(
            "POST",
            LABELS,
            &json!([label("work/stage/shape"), label("work/stage/spec")]),
        ),
        reply(
            ISSUE,
            200,
            &observed(&["work/stage/shape", "work/stage/spec"], "Words"),
        ),
        sent("DELETE", SHAPE, &json!([label("work/stage/spec")])),
        reply(ISSUE, 200, &observed(&["work/stage/spec"], "Words")),
    ])
    .await?;
    let editor = Editor::new(&base)?;
    let handle = editor.operations.handle()?;
    let written = editor
        .edit(handle, &native(&before, "Words"), move_to("spec"))
        .await?;
    assert_eq!(written.labels, ["work/stage/spec"]);
    assert_eq!(
        bodies(&served.await??),
        [json!({"labels": ["work/stage/spec"]})]
    );
    Ok(())
}

#[tokio::test]
async fn a_label_write_whose_reply_is_lost_is_found_applied_on_retry_and_sent_once()
-> anyhow::Result<()> {
    let before = ["work/stage/shape"];
    let (base, served) = exchanges(vec![
        reply(ISSUE, 200, &observed(&before, "Words")),
        lost("POST", LABELS),
        reply(
            ISSUE,
            200,
            &observed(&["work/stage/shape", "work/stage/spec"], "Words"),
        ),
        sent("DELETE", SHAPE, &json!([label("work/stage/spec")])),
        reply(ISSUE, 200, &observed(&["work/stage/spec"], "Words")),
    ])
    .await?;
    let editor = Editor::new(&base)?;
    let handle = editor.operations.handle()?;
    let seen = native(&before, "Words");
    assert!(matches!(
        editor.edit(handle, &seen, move_to("spec")).await,
        Err(WriteError::Unsettled(unknown)) if unknown == handle
    ));
    let written = editor.edit(handle, &seen, move_to("spec")).await?;
    assert_eq!(written.labels, ["work/stage/spec"]);
    assert_eq!(
        bodies(&served.await??),
        [json!({"labels": ["work/stage/spec"]})]
    );
    Ok(())
}

#[tokio::test]
async fn a_body_edit_keeps_the_project_data_footer() -> anyhow::Result<()> {
    let old = format!("Old words\n\n{FOOTER}");
    let new = format!("New words\n\n{FOOTER}");
    let (base, served) = exchanges(vec![
        reply(ISSUE, 200, &observed(&[], &old)),
        sent("PATCH", ISSUE, &observed(&[], &new)),
        reply(ISSUE, 200, &observed(&[], &new)),
    ])
    .await?;
    let editor = Editor::new(&base)?;
    let handle = editor.operations.handle()?;
    editor
        .edit(
            handle,
            &native(&[], &old),
            Vec::from([Mutation::Body(String::from("New words"))]),
        )
        .await?;
    assert_eq!(bodies(&served.await??), [json!({ "body": new })]);
    Ok(())
}

#[tokio::test]
async fn an_issue_changed_since_it_was_read_is_refused_before_any_write()
-> anyhow::Result<()> {
    let (base, served) = exchanges(vec![reply(
        ISSUE,
        200,
        &observed(&["work/stage/shape", "Edited by a person"], "Words"),
    )])
    .await?;
    let editor = Editor::new(&base)?;
    let handle = editor.operations.handle()?;
    assert!(matches!(
        editor
            .edit(
                handle,
                &native(&["work/stage/shape"], "Words"),
                move_to("spec")
            )
            .await,
        Err(WriteError::Operation(Error::Refused(Refusal::Conflict)))
    ));
    assert_eq!(editor.operations.status(handle)?, None);
    assert_eq!(bodies(&served.await??), Vec::<Value>::new());
    Ok(())
}

#[tokio::test]
async fn setting_parent_through_an_issue_edit_adds_a_sub_issue_and_leaves_the_body_alone()
-> anyhow::Result<()> {
    let mut parent = issue(140, 9140);
    parent["sub_issues_summary"] =
        json!({"total": 3, "completed": 0, "percent_completed": 0});
    let mut adopted = observed(&[], "Words");
    adopted["parent_issue_url"] = json!("{api}/repos/owner/work/issues/140");
    let (base, served) = exchanges(vec![
        reply(ISSUE, 200, &observed(&[], "Words")),
        reply(ISSUE, 200, &observed(&[], "Words")),
        reply("/repos/owner/work/issues/140", 200, &parent),
        reply(
            "/repos/owner/work/issues/146/sub_issues?per_page=100",
            200,
            &json!([]),
        ),
        Exchange {
            status: 201,
            ..sent("POST", "/repos/owner/work/issues/140/sub_issues", &parent)
        },
        reply(ISSUE, 200, &adopted),
        reply(ISSUE, 200, &adopted),
    ])
    .await?;
    let editor = Editor::new(&base)?;
    let handle = editor.operations.handle()?;
    let target = lookup("140")?;
    editor
        .edit(
            handle,
            &native(&[], "Words"),
            Vec::from([Mutation::Relate {
                relation: String::from("parent"),
                many: false,
                targets: Vec::from([home::IssueKey {
                    project: target.project,
                    id: target.id,
                }]),
            }]),
        )
        .await?;
    assert_eq!(bodies(&served.await??), [json!({"sub_issue_id": 9146})]);
    Ok(())
}
