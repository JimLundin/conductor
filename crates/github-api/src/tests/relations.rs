use super::comments::Writer;
use super::tracking::{issue, lookup, reply};
use super::{Exchange, exchanges};
use home::issue::{RelationChange, RelationRequest};
use home::operations::{Handle, Progress, Step};
use home::{IssueId, IssueKey, ProjectId};
use project_runtime::WriteError;
use serde_json::{Value, json};

const CHILD: &str = "/repos/owner/work/issues/146";
const PARENT: &str = "/repos/owner/work/issues/140";
const ORPHAN: &str = "/repos/owner/work/issues/139/sub_issue";
const CHILDREN: &str = "/repos/owner/work/issues/146/sub_issues?per_page=100";
const ADOPT: &str = "/repos/owner/work/issues/140/sub_issues";
const BLOCKERS: &str =
    "/repos/owner/work/issues/146/dependencies/blocked_by?per_page=100";
const BLOCK: &str = "/repos/owner/work/issues/146/dependencies/blocked_by";
const UNBLOCK: &str =
    "/repos/owner/work/issues/146/dependencies/blocked_by/9150";
const OLD_BLOCKER: &str = "/repos/owner/work/issues/150";
const NEW_BLOCKER: &str = "/repos/owner/work/issues/151";
const PROSE: &str = "Visible human text";

fn child(parent: Option<u64>, body: &str) -> Value {
    let mut value = issue(146, 9146);
    value["body"] = json!(body);
    value["parent_issue_url"] = json!(
        parent
            .map(|number| format!("{{api}}/repos/owner/work/issues/{number}"))
    );
    value
}

fn blocked(by: &[u64]) -> Value {
    let mut value = child(None, PROSE);
    value["issue_dependencies_summary"]["total_blocked_by"] = json!(by.len());
    value
}

fn parent(sub_issues: u32) -> Value {
    let mut value = issue(140, 9140);
    value["sub_issues_summary"] =
        json!({"total": sub_issues, "completed": 0, "percent_completed": 0});
    value
}

fn sent(method: &'static str, path: &'static str, status: u16) -> Exchange {
    Exchange {
        method,
        ..reply(path, status, &parent(1))
    }
}

fn lost(method: &'static str, path: &'static str) -> Exchange {
    Exchange {
        lost: true,
        ..sent(method, path, 201)
    }
}

fn here(id: &str) -> anyhow::Result<IssueKey> {
    Ok(IssueKey {
        project: ProjectId::new(1),
        id: IssueId::new(id).ok_or_else(|| anyhow::anyhow!("no id"))?,
    })
}

fn handle(id: u128) -> Handle {
    Handle {
        owner: [7; 32],
        id: home::issue::EditId(id),
    }
}

impl Writer {
    async fn setting(
        &self,
        relation: &str,
        targets: Vec<IssueKey>,
    ) -> anyhow::Result<RelationRequest> {
        let issue = lookup("146")?;
        let seen = self
            .registry
            .project(&self.backends)
            .read_issue(issue.clone())
            .await?
            .revision
            .ok_or_else(|| anyhow::anyhow!("no revision"))?;
        Ok(RelationRequest {
            issue,
            change: RelationChange {
                relation: String::from(relation),
                targets,
            },
            seen,
        })
    }

    async fn relate(
        &self,
        handle: Handle,
        request: RelationRequest,
    ) -> Result<String, WriteError> {
        self.registry
            .project(&self.backends)
            .write_relation(&self.operations, handle, request)
            .await
    }

    fn recorded(&self, handle: Handle) -> anyhow::Result<RelationRequest> {
        let record = self
            .operations
            .status(handle)?
            .ok_or_else(|| anyhow::anyhow!("no record"))?;
        let home::operations::Work::Relate { issue, change } =
            record.intent.work
        else {
            return Err(anyhow::anyhow!("not a relation write"));
        };
        Ok(RelationRequest {
            issue: home::issue::Lookup {
                project: record.intent.project,
                id: issue,
            },
            change,
            seen: record.intent.expected,
        })
    }
}

fn bodies(served: &[hyper::body::Bytes]) -> Vec<Value> {
    served
        .iter()
        .filter(|body| !body.is_empty())
        .filter_map(|body| serde_json::from_slice(body).ok())
        .collect()
}

#[tokio::test]
async fn setting_parent_to_an_issue_of_the_same_repository_adds_a_sub_issue_and_writes_no_footer()
-> anyhow::Result<()> {
    let (base, served) = exchanges(vec![
        reply(CHILD, 200, &child(None, PROSE)),
        reply(CHILD, 200, &child(None, PROSE)),
        reply(PARENT, 200, &parent(3)),
        reply(CHILDREN, 200, &json!([])),
        sent("POST", ADOPT, 201),
        reply(CHILD, 200, &child(Some(140), PROSE)),
    ])
    .await?;
    let writer = Writer::new(&base)?;
    let handle = handle(21);
    writer
        .relate(handle, writer.setting("parent", vec![here("140")?]).await?)
        .await?;
    assert_eq!(writer.progress(handle)?, Progress::Completed);
    assert_eq!(bodies(&served.await??), [json!({"sub_issue_id": 9146})]);
    Ok(())
}

#[tokio::test]
async fn setting_parent_to_an_issue_of_another_project_writes_the_footer_and_sends_no_sub_issue_request()
-> anyhow::Result<()> {
    let elsewhere = IssueKey {
        project: ProjectId::new(2),
        id: IssueId::new("0v2csjdj").ok_or_else(|| anyhow::anyhow!("no id"))?,
    };
    let footed = format!(
        "{PROSE}\n\n```project-data\nschema = 1\n\n[relations]\nparent = \"{elsewhere}\"\n```\n"
    );
    let (base, served) = exchanges(vec![
        reply(CHILD, 200, &child(None, PROSE)),
        reply(CHILD, 200, &child(None, PROSE)),
        sent("PATCH", CHILD, 200),
        reply(CHILD, 200, &child(None, &footed)),
    ])
    .await?;
    let writer = Writer::new(&base)?;
    let handle = handle(22);
    writer
        .relate(handle, writer.setting("parent", vec![elsewhere]).await?)
        .await?;
    assert_eq!(writer.progress(handle)?, Progress::Completed);
    assert_eq!(bodies(&served.await??), [json!({"body": footed})]);
    Ok(())
}

#[tokio::test]
async fn an_uncertain_relation_write_is_recorded_and_settles_once_the_sub_issue_is_read()
-> anyhow::Result<()> {
    let (base, served) = exchanges(vec![
        reply(CHILD, 200, &child(None, PROSE)),
        reply(CHILD, 200, &child(None, PROSE)),
        reply(PARENT, 200, &parent(3)),
        reply(CHILDREN, 200, &json!([])),
        lost("POST", ADOPT),
        reply(CHILD, 200, &child(None, PROSE)),
        reply(PARENT, 200, &parent(3)),
        reply(CHILDREN, 200, &json!([])),
        reply(CHILD, 200, &child(Some(140), PROSE)),
    ])
    .await?;
    let writer = Writer::new(&base)?;
    let handle = handle(23);
    let awaiting = Progress::AwaitingEvidence(Step::Relate(String::from(
        "sub-issue of 140",
    )));
    assert!(matches!(
        writer
            .relate(handle, writer.setting("parent", vec![here("140")?]).await?)
            .await,
        Err(WriteError::Unknown(unknown)) if unknown == handle
    ));
    assert_eq!(writer.progress(handle)?, awaiting);
    let request = writer.recorded(handle)?;
    assert!(matches!(
        writer.relate(handle, request.clone()).await,
        Err(WriteError::Unknown(_))
    ));
    assert_eq!(writer.progress(handle)?, awaiting);
    writer.relate(handle, request).await?;
    assert_eq!(writer.progress(handle)?, Progress::Completed);
    assert_eq!(bodies(&served.await??), [json!({"sub_issue_id": 9146})]);
    Ok(())
}

#[tokio::test]
async fn moving_a_sub_issue_to_another_parent_replaces_its_parent()
-> anyhow::Result<()> {
    let (base, served) = exchanges(vec![
        reply(CHILD, 200, &child(Some(139), PROSE)),
        reply(CHILD, 200, &child(Some(139), PROSE)),
        reply(PARENT, 200, &parent(3)),
        reply(CHILDREN, 200, &json!([])),
        sent("POST", ADOPT, 201),
        reply(CHILD, 200, &child(Some(140), PROSE)),
    ])
    .await?;
    let writer = Writer::new(&base)?;
    writer
        .relate(
            handle(24),
            writer.setting("parent", vec![here("140")?]).await?,
        )
        .await?;
    assert_eq!(
        bodies(&served.await??),
        [json!({"sub_issue_id": 9146, "replace_parent": true})]
    );
    Ok(())
}

#[tokio::test]
async fn clearing_a_native_parent_removes_the_sub_issue() -> anyhow::Result<()>
{
    let (base, served) = exchanges(vec![
        reply(CHILD, 200, &child(Some(139), PROSE)),
        reply(CHILD, 200, &child(Some(139), PROSE)),
        sent("DELETE", ORPHAN, 200),
        reply(CHILD, 200, &child(None, PROSE)),
    ])
    .await?;
    let writer = Writer::new(&base)?;
    writer
        .relate(handle(25), writer.setting("parent", Vec::new()).await?)
        .await?;
    assert_eq!(bodies(&served.await??), [json!({"sub_issue_id": 9146})]);
    Ok(())
}

#[tokio::test]
async fn a_parent_holding_100_sub_issues_is_refused_before_any_write()
-> anyhow::Result<()> {
    let (base, served) = exchanges(vec![
        reply(CHILD, 200, &child(None, PROSE)),
        reply(CHILD, 200, &child(None, PROSE)),
        reply(PARENT, 200, &parent(100)),
        reply(CHILDREN, 200, &json!([])),
    ])
    .await?;
    let writer = Writer::new(&base)?;
    let handle = handle(26);
    assert!(matches!(
        writer
            .relate(handle, writer.setting("parent", vec![here("140")?]).await?)
            .await,
        Err(WriteError::Unheld(_))
    ));
    assert_eq!(writer.progress(handle)?, Progress::Ready);
    assert_eq!(bodies(&served.await??), Vec::<Value>::new());
    Ok(())
}

#[tokio::test]
async fn a_sub_issue_github_finds_invalid_leaves_the_operation_ready()
-> anyhow::Result<()> {
    let (base, served) = exchanges(vec![
        reply(CHILD, 200, &child(None, PROSE)),
        reply(CHILD, 200, &child(None, PROSE)),
        reply(PARENT, 200, &parent(3)),
        reply(CHILDREN, 200, &json!([])),
        Exchange {
            body: json!({"message": "Validation Failed"}).to_string(),
            ..sent("POST", ADOPT, 422)
        },
    ])
    .await?;
    let writer = Writer::new(&base)?;
    let handle = handle(27);
    assert!(matches!(
        writer
            .relate(handle, writer.setting("parent", vec![here("140")?]).await?)
            .await,
        Err(WriteError::Invalid)
    ));
    assert_eq!(writer.progress(handle)?, Progress::Ready);
    assert_eq!(bodies(&served.await??).len(), 1);
    Ok(())
}

#[tokio::test]
async fn setting_blocked_by_adds_and_removes_dependencies_by_the_blockers_ids()
-> anyhow::Result<()> {
    let blocker = |number: u64| json!({"url": format!("{{api}}/repos/owner/work/issues/{number}")});
    let target = |number: u64| issue(number, 9000 + number);
    let (base, served) = exchanges(vec![
        reply(CHILD, 200, &blocked(&[150])),
        reply(BLOCKERS, 200, &json!([blocker(150)])),
        reply(CHILD, 200, &blocked(&[150])),
        reply(BLOCKERS, 200, &json!([blocker(150)])),
        reply(NEW_BLOCKER, 200, &target(151)),
        reply(OLD_BLOCKER, 200, &target(150)),
        sent("POST", BLOCK, 201),
        reply(CHILD, 200, &blocked(&[150, 151])),
        reply(BLOCKERS, 200, &json!([blocker(150), blocker(151)])),
        reply(OLD_BLOCKER, 200, &target(150)),
        sent("DELETE", UNBLOCK, 200),
        reply(CHILD, 200, &blocked(&[151])),
        reply(BLOCKERS, 200, &json!([blocker(151)])),
    ])
    .await?;
    let writer = Writer::new(&base)?;
    let handle = handle(28);
    writer
        .relate(
            handle,
            writer.setting("blocked_by", vec![here("151")?]).await?,
        )
        .await?;
    assert_eq!(writer.progress(handle)?, Progress::Completed);
    assert_eq!(bodies(&served.await??), [json!({"issue_id": 9151})]);
    Ok(())
}

#[tokio::test]
async fn a_parent_kept_in_the_footer_moves_to_a_sub_issue() -> anyhow::Result<()>
{
    let footed = format!(
        "{PROSE}\n\n```project-data\nschema = 1\n\n[relations]\nparent = \"140\"\n```\n"
    );
    let (base, served) = exchanges(vec![
        reply(CHILD, 200, &child(None, &footed)),
        reply(CHILD, 200, &child(None, &footed)),
        reply(PARENT, 200, &parent(3)),
        reply(CHILDREN, 200, &json!([])),
        sent("POST", ADOPT, 201),
        reply(CHILD, 200, &child(Some(140), &footed)),
        sent("PATCH", CHILD, 200),
        reply(CHILD, 200, &child(Some(140), PROSE)),
    ])
    .await?;
    let writer = Writer::new(&base)?;
    let handle = handle(29);
    writer
        .relate(handle, writer.setting("parent", vec![here("140")?]).await?)
        .await?;
    assert_eq!(writer.progress(handle)?, Progress::Completed);
    assert_eq!(
        bodies(&served.await??),
        [json!({"sub_issue_id": 9146}), json!({"body": PROSE})]
    );
    Ok(())
}
