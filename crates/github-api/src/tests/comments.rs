use super::tracking::{author, lookup, reply, tracker};
use super::{Exchange, exchanges};
use home::backend::{Backends, Capability, CredentialState, Refusal};
use home::issue::{Comment, CommentRequest, CommentWrite};
use home::operations::{Handle, Progress, Step};
use project_runtime::operations::Operations;
use project_runtime::{Registration, Registry, WriteError};
use serde_json::{Value, json};
use std::sync::Arc;

pub(super) const LISTED: &str =
    "/repos/owner/work/issues/146/comments?per_page=100&page=1";
pub(super) const CREATED: &str = "/repos/owner/work/issues/146/comments";
const EDITED: &str = "/repos/owner/work/issues/comments/2";

pub(super) fn comment(id: u64, body: &str, updated: &str) -> Value {
    json!({
        "id": id, "node_id": format!("IC_{id}"),
        "url": format!("{{api}}/repos/owner/work/issues/comments/{id}"),
        "html_url": format!("https://github.com/owner/work/issues/146#issuecomment-{id}"),
        "issue_url": "{api}/repos/owner/work/issues/146",
        "body": body, "user": author(),
        "created_at": "2026-11-01T10:00:00Z", "updated_at": updated
    })
}

pub(super) fn human() -> Value {
    comment(1, "A person's note", "2026-11-01T10:00:00Z")
}

pub(super) fn sent(
    method: &'static str,
    path: &'static str,
    body: &Value,
) -> Exchange {
    Exchange {
        method,
        status: if method == "POST" { 201 } else { 200 },
        ..reply(path, 200, body)
    }
}

pub(super) fn lost(method: &'static str, path: &'static str) -> Exchange {
    Exchange {
        lost: true,
        ..sent(method, path, &json!({}))
    }
}

pub(super) struct Writer {
    _dir: tempfile::TempDir,
    pub(super) operations: Operations,
    pub(super) registry: Registry,
    pub(super) backends: Backends,
}

impl Writer {
    pub(super) fn new(base: &str) -> anyhow::Result<Self> {
        let tracker = tracker(base)?;
        Self::with(
            tracker,
            vec![Capability::ReadIssues, Capability::WriteIssues],
        )
    }

    fn with(
        tracker: crate::Tracker,
        capabilities: Vec<Capability>,
    ) -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        let operations = Operations::adopt(dir.path(), || Ok([7; 32]))?;
        let selection = super::selection("owner/work");
        let mut registry = Registry::default();
        registry
            .register_tracker(
                Registration {
                    selection: selection.clone(),
                    capabilities,
                    credential: CredentialState::Available,
                },
                Arc::new(tracker),
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

    async fn seen(&self, write: &CommentWrite) -> anyhow::Result<String> {
        let comments = self
            .registry
            .project(&self.backends)
            .comments(lookup("146")?)
            .await?;
        Ok(write.precondition(&comments))
    }

    async fn write(
        &self,
        handle: Handle,
        request: CommentRequest,
    ) -> Result<Comment, WriteError> {
        self.registry
            .project(&self.backends)
            .write_comment(&self.operations, handle, request)
            .await
    }

    pub(super) fn progress(&self, handle: Handle) -> anyhow::Result<Progress> {
        Ok(self
            .operations
            .status(handle)?
            .ok_or_else(|| anyhow::anyhow!("no record"))?
            .progress()
            .clone())
    }
}

fn marked(handle: Handle) -> String {
    format!("The canvas\n<!-- {} -->", handle.marker())
}

async fn create(
    writer: &Writer,
    handle: Handle,
) -> anyhow::Result<(CommentRequest, String)> {
    let body = marked(handle);
    let write = CommentWrite::Create { body: body.clone() };
    let seen = writer.seen(&write).await?;
    Ok((
        CommentRequest {
            issue: lookup("146")?,
            write,
            seen,
        },
        body,
    ))
}

pub(super) fn posted(bodies: &[hyper::body::Bytes]) -> Vec<String> {
    bodies
        .iter()
        .filter(|body| !body.is_empty())
        .filter_map(|body| serde_json::from_slice::<Value>(body).ok())
        .filter_map(|value| value["body"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test]
async fn a_create_sends_one_request_whose_body_holds_the_operation_marker()
-> anyhow::Result<()> {
    let handle = Handle {
        owner: [7; 32],
        id: home::issue::EditId(11),
    };
    let body = marked(handle);
    let made = comment(2, &body, "2026-11-02T10:00:00Z");
    let (base, served) = exchanges(vec![
        reply(LISTED, 200, &json!([human()])),
        reply(LISTED, 200, &json!([human()])),
        sent("POST", CREATED, &made),
        reply(LISTED, 200, &json!([human(), made])),
    ])
    .await?;
    let writer = Writer::new(&base)?;
    let (request, body) = create(&writer, handle).await?;
    let written = writer.write(handle, request).await?;
    assert_eq!(written.id, "2");
    assert_eq!(writer.progress(handle)?, Progress::Completed);
    assert_eq!(posted(&served.await??), [body]);
    Ok(())
}

#[tokio::test]
async fn an_edit_changes_only_the_named_comment() -> anyhow::Result<()> {
    let handle = Handle {
        owner: [7; 32],
        id: home::issue::EditId(12),
    };
    let old = comment(2, "The old canvas", "2026-11-01T10:00:00Z");
    let body = marked(handle);
    let new = comment(2, &body, "2026-11-02T10:00:00Z");
    let (base, served) = exchanges(vec![
        reply(LISTED, 200, &json!([human(), old.clone()])),
        reply(LISTED, 200, &json!([human(), old])),
        sent("PATCH", EDITED, &new),
        reply(LISTED, 200, &json!([human(), new])),
    ])
    .await?;
    let writer = Writer::new(&base)?;
    let write = CommentWrite::Edit {
        comment: String::from("2"),
        body: body.clone(),
    };
    let seen = writer.seen(&write).await?;
    let edited = writer
        .write(
            handle,
            CommentRequest {
                issue: lookup("146")?,
                write,
                seen,
            },
        )
        .await?;
    assert_eq!((edited.id.as_str(), edited.body), ("2", body.clone()));
    assert_eq!(posted(&served.await??), [body]);
    Ok(())
}

#[tokio::test]
async fn a_create_whose_reply_is_lost_finds_its_marker_on_retry_and_leaves_one_comment()
-> anyhow::Result<()> {
    let handle = Handle {
        owner: [7; 32],
        id: home::issue::EditId(13),
    };
    let made = comment(2, &marked(handle), "2026-11-02T10:00:00Z");
    let (base, served) = exchanges(vec![
        reply(LISTED, 200, &json!([human()])),
        reply(LISTED, 200, &json!([human()])),
        lost("POST", CREATED),
        reply(LISTED, 200, &json!([human(), made])),
    ])
    .await?;
    let writer = Writer::new(&base)?;
    let (request, body) = create(&writer, handle).await?;
    assert!(matches!(
        writer.write(handle, request.clone()).await,
        Err(WriteError::Unknown(unknown)) if unknown == handle
    ));
    let found = writer.write(handle, request).await?;
    assert_eq!(found.id, "2");
    assert_eq!(posted(&served.await??), [body]);
    Ok(())
}

#[tokio::test]
async fn an_uncertain_create_is_recorded_and_resolves_once_its_marker_is_found()
-> anyhow::Result<()> {
    let handle = Handle {
        owner: [7; 32],
        id: home::issue::EditId(14),
    };
    let made = comment(2, &marked(handle), "2026-11-02T10:00:00Z");
    let (base, served) = exchanges(vec![
        reply(LISTED, 200, &json!([human()])),
        reply(LISTED, 200, &json!([human()])),
        lost("POST", CREATED),
        reply(LISTED, 200, &json!([human()])),
        reply(LISTED, 200, &json!([human(), made])),
    ])
    .await?;
    let writer = Writer::new(&base)?;
    let (request, body) = create(&writer, handle).await?;
    assert!(writer.write(handle, request.clone()).await.is_err());
    assert_eq!(
        writer.progress(handle)?,
        Progress::AwaitingEvidence(Step::WriteComment)
    );
    assert!(matches!(
        writer.write(handle, request.clone()).await,
        Err(WriteError::Unknown(_))
    ));
    assert_eq!(
        writer.progress(handle)?,
        Progress::AwaitingEvidence(Step::WriteComment)
    );
    writer.write(handle, request).await?;
    let record = writer
        .operations
        .status(handle)?
        .ok_or_else(|| anyhow::anyhow!("no record"))?;
    assert_eq!(record.progress(), &Progress::Completed);
    assert_eq!(
        record
            .confirmed()
            .iter()
            .map(|confirmed| confirmed.identity.as_deref())
            .collect::<Vec<_>>(),
        [Some("2")]
    );
    assert_eq!(posted(&served.await??), [body]);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn a_credential_without_issue_writes_fails_the_write_preflight_and_sends_nothing()
-> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("project-credentials.toml");
    let entry = |writes: &str| {
        format!(
            "[\"host/work\"]\nrepository = \"owner/work\"\n{writes}\n[\"host/work\".source]\nkind = \"fine_grained\"\ntoken = \"fixture\"\n"
        )
    };
    std::fs::write(&path, entry("write_issues = true"))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    let credentials = crate::HostCredentials::new(dir.path());
    let selection = super::selection("owner/work");
    let flow =
        home::flow::load(include_str!("../../../home/src/flow/test-flow.toml"))
            .map_err(anyhow::Error::msg)?;
    let mapping =
        github::configuration::load("", &flow).map_err(anyhow::Error::msg)?;
    let writing = credentials
        .connect_at(&selection, "http://127.0.0.1:9")?
        .tracker(
            flow.clone(),
            mapping.clone(),
            home::tracker::Ids::default(),
            github::access::Repositories::default(),
        );
    assert_eq!(
        writing.capabilities(),
        [Capability::ReadIssues, Capability::WriteIssues]
    );
    std::fs::write(&path, entry(""))?;
    let (base, served) = exchanges(Vec::new()).await?;
    let reading = credentials.connect_at(&selection, &base)?.tracker(
        flow,
        mapping,
        home::tracker::Ids::default(),
        github::access::Repositories::default(),
    );
    let capabilities = reading.capabilities();
    let writer = Writer::with(reading, capabilities)?;
    let handle = writer.operations.handle()?;
    let request = CommentRequest {
        issue: lookup("146")?,
        write: CommentWrite::Create {
            body: marked(handle),
        },
        seen: String::new(),
    };
    assert!(matches!(
        writer.write(handle, request).await,
        Err(WriteError::Refused(Refusal::CapabilityUnavailable {
            capability: Capability::WriteIssues
        }))
    ));
    assert_eq!(writer.operations.status(handle)?, None);
    assert_eq!(served.await??, Vec::<hyper::body::Bytes>::new());
    Ok(())
}
