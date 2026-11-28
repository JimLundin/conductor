use std::collections::BTreeMap;
use std::error::Error;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

use home::backend::{Backends, Capability, CredentialState, Selection};
use home::issue::{
    Freshness, ListPage, Locator, Lookup, Observation, Page, ReadFailure,
    ReadMode, Unreadable,
};
use home::{IssueId, ProjectId, UnixMillis};
use project_runtime::{
    IssueCompletion, IssueTracker, Registration, Registry, issues::IssueCache,
};

struct Replies<T>(mpsc::UnboundedSender<oneshot::Sender<T>>);

impl<T: Send + 'static> Replies<T> {
    fn new(values: Vec<T>) -> Self {
        let (send, mut requests) =
            mpsc::unbounded_channel::<oneshot::Sender<T>>();
        tokio::spawn(async move {
            for value in values {
                let Some(reply) = requests.recv().await else {
                    break;
                };
                let _sent = reply.send(value);
            }
        });
        Self(send)
    }

    async fn next(&self) -> Result<T, ReadFailure> {
        let (send, receive) = oneshot::channel();
        self.0
            .send(send)
            .map_err(|_| ReadFailure::InvalidResponse)?;
        receive.await.map_err(|_| ReadFailure::InvalidResponse)
    }
}

type Listed = (Option<&'static str>, Result<Page, ReadFailure>);

struct Script {
    pages: Replies<Listed>,
    reads: Replies<Result<Observation, ReadFailure>>,
}

impl Script {
    fn pages<const N: usize>(items: [Listed; N]) -> Arc<Self> {
        Arc::new(Self {
            pages: Replies::new(Vec::from(items)),
            reads: Replies::new(Vec::new()),
        })
    }
    fn reads<const N: usize>(
        items: [Result<Observation, ReadFailure>; N],
    ) -> Arc<Self> {
        Arc::new(Self {
            pages: Replies::new(Vec::new()),
            reads: Replies::new(Vec::from(items)),
        })
    }
}

impl IssueTracker for Script {
    fn read(&self, _: Lookup) -> IssueCompletion<'_, Observation> {
        Box::pin(async { self.reads.next().await? })
    }
    fn list(&self, request: ListPage) -> IssueCompletion<'_, Page> {
        Box::pin(async move {
            let (expected, reply) = self.pages.next().await?;
            if expected != request.continuation.as_deref() {
                return Err(ReadFailure::InvalidResponse);
            }
            reply
        })
    }
}

const fn project() -> ProjectId {
    ProjectId::new(32)
}
fn item(key: &str, title: &str) -> Result<Observation, Box<dyn Error>> {
    let mut record = Observation::invalid(Unreadable {
        id: IssueId::new(key).ok_or("fixture identity")?,
        project: project(),
        title: Some(title.into()),
        body: Some("human text".into()),
        created: Some(UnixMillis(10)),
        reason: "unmapped label".into(),
    });
    record.locator = Some(Locator {
        repository: "example".into(),
        url: None,
        addresses: BTreeMap::new(),
    });
    Ok(record)
}

fn lookup(key: &str) -> Result<Lookup, Box<dyn Error>> {
    Ok(Lookup {
        project: project(),
        id: IssueId::new(key).ok_or("fixture identity")?,
    })
}

fn setup(
    adapter: Arc<dyn IssueTracker>,
    resource: &str,
) -> Result<(Registry, Backends), Box<dyn Error>> {
    let mut registry = Registry::default();
    let selection = Selection {
        adapter: "example".into(),
        resource: resource.into(),
        credential: None,
        options: String::new(),
    };
    registry
        .register_tracker(
            Registration {
                selection: selection.clone(),
                capabilities: vec![Capability::ReadIssues],
                credential: CredentialState::NotRequired,
            },
            adapter,
        )
        .map_err(|err| err.label())?;
    Ok((
        registry,
        Backends {
            tracker: Some(selection),
            ..Backends::default()
        },
    ))
}

#[tokio::test]
async fn failed_second_page_keeps_the_last_complete_snapshot_across_process_restarts()
-> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let flow =
        home::flow::load(include_str!("../../home/src/flow/test-flow.toml"))?;
    let cache = IssueCache::new(dir.path(), &flow);
    let first = item("1", "old issue")?;
    let adapter = Script::pages([
        (
            None,
            Ok(Page {
                records: vec![first.clone()],
                next: None,
            }),
        ),
        (
            None,
            Ok(Page {
                records: vec![item("2", "partial refresh")?],
                next: Some("2".into()),
            }),
        ),
        (Some("2"), Err(ReadFailure::RateLimited)),
    ]);
    let (registry, backends) = setup(adapter.clone(), "example")?;
    let service = registry.project(&backends);
    let initial = service.issues(&cache, project(), ReadMode::Refresh).await?;
    assert!(initial.status.complete);
    assert_eq!(initial.status.freshness, Freshness::Fresh);
    assert_eq!(initial.records.len(), 1);
    let failed = service.issues(&cache, project(), ReadMode::Refresh).await?;
    assert!(failed.status.complete);
    assert_eq!(failed.status.freshness, Freshness::Stale);
    assert_eq!(failed.status.failure, Some(ReadFailure::RateLimited));
    assert_eq!(failed.records, initial.records);
    let (registry, backends) = setup(Script::pages([]), "example")?;
    let disk = registry
        .project(&backends)
        .issues(
            &IssueCache::new(dir.path(), &flow),
            project(),
            ReadMode::Cached,
        )
        .await?;
    assert_eq!(disk.records, initial.records);
    assert_eq!(disk.status.observed_at, initial.status.observed_at);
    assert_eq!(disk.status.freshness, Freshness::Stale);
    Ok(())
}

#[tokio::test]
async fn a_missing_or_mismatched_read_keeps_the_previous_identity_and_text()
-> Result<(), Box<dyn Error>> {
    use home::issue::IssueState;
    let dir = tempfile::tempdir()?;
    let flow =
        home::flow::load(include_str!("../../home/src/flow/test-flow.toml"))?;
    let cache = IssueCache::new(dir.path(), &flow);
    let first = item("1", "human title")?;
    let replacement = item("2", "replacement")?;
    let mut deleted = first.clone();
    if let IssueState::Invalid(mut tombstone) = deleted.state {
        tombstone.title = None;
        tombstone.body = None;
        deleted.state = IssueState::Deleted(tombstone);
    }
    deleted.locator = None;
    let adapter = Script::reads([
        Ok(first.clone()),
        Err(ReadFailure::Unavailable),
        Ok(replacement),
        Ok(deleted),
    ]);
    let (registry, backends) = setup(adapter, "example")?;
    let service = registry.project(&backends);
    let initial = service
        .issue(&cache, lookup("1")?, ReadMode::Refresh)
        .await?;
    assert_eq!(
        initial.record.as_ref().map(Observation::id),
        Some(first.id())
    );
    let missing = service
        .issue(&cache, lookup("1")?, ReadMode::Refresh)
        .await?;
    assert_eq!(missing.status.freshness, Freshness::Stale);
    assert_eq!(missing.status.failure, Some(ReadFailure::Unavailable));
    assert!(
        matches!(missing.record.map(|o| o.state), Some(IssueState::Unavailable(previous)) if previous.title.as_deref()==Some("human title") && previous.body.as_deref()==Some("human text"))
    );
    let conflict = service
        .issue(&cache, lookup("1")?, ReadMode::Refresh)
        .await?;
    assert_eq!(conflict.status.failure, Some(ReadFailure::IdentityConflict));
    assert_eq!(
        conflict.record.as_ref().map(Observation::id),
        Some(first.id())
    );
    let confirmed = service
        .issue(&cache, lookup("1")?, ReadMode::Refresh)
        .await?;
    assert_eq!(confirmed.status.freshness, Freshness::Fresh);
    let record = confirmed.record.ok_or("missing tombstone")?;
    assert_eq!(record.locator, first.locator);
    assert!(
        matches!(record.state, IssueState::Deleted(previous) if previous.title.as_deref() == Some("human title"))
    );
    Ok(())
}

#[tokio::test]
async fn project_tracker_and_flow_changes_do_not_expose_a_different_cached_projection()
-> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let flow =
        home::flow::load(include_str!("../../home/src/flow/test-flow.toml"))?;
    let cache = IssueCache::new(dir.path(), &flow);
    let adapter = Script::pages([(
        None,
        Ok(Page {
            records: vec![item("1", "private issue")?],
            next: None,
        }),
    )]);
    let (registry, backends) = setup(adapter, "example")?;
    let service = registry.project(&backends);
    service.issues(&cache, project(), ReadMode::Refresh).await?;
    let mut changed = flow.clone();
    changed.default_kind = "new default".into();
    let alternate = IssueCache::new(dir.path(), &changed);
    let (new_registry, new_backends) =
        setup(Script::pages([]), "another-repository")?;
    for reading in [
        service
            .issues(&alternate, project(), ReadMode::Cached)
            .await?,
        service
            .issues(&cache, ProjectId::new(28), ReadMode::Cached)
            .await?,
        new_registry
            .project(&new_backends)
            .issues(&cache, project(), ReadMode::Cached)
            .await?,
    ] {
        assert_eq!(reading.status.freshness, Freshness::Unknown);
        assert!(!reading.status.complete);
        assert_eq!(reading.records, []);
    }
    Ok(())
}

struct SlowFirst {
    started: tokio::sync::Notify,
    finish: tokio::sync::Notify,
    older: Observation,
}

impl SlowFirst {
    async fn next(&self) -> Observation {
        self.started.notify_one();
        self.finish.notified().await;
        self.older.clone()
    }
}

impl IssueTracker for SlowFirst {
    fn read(&self, _: Lookup) -> IssueCompletion<'_, Observation> {
        Box::pin(async { Ok(self.next().await) })
    }
    fn list(&self, _: ListPage) -> IssueCompletion<'_, Page> {
        Box::pin(async {
            Ok(Page {
                records: vec![self.next().await],
                next: None,
            })
        })
    }
}

#[tokio::test]
async fn an_earlier_refresh_finishing_last_cannot_undo_a_direct_tracker_edit()
-> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let flow =
        home::flow::load(include_str!("../../home/src/flow/test-flow.toml"))?;
    let cache = IssueCache::new(dir.path(), &flow);
    let adapter = Arc::new(SlowFirst {
        started: tokio::sync::Notify::new(),
        finish: tokio::sync::Notify::new(),
        older: item("42", "before edit")?,
    });
    let (registry, backends) = setup(adapter.clone(), "example")?;
    let service = registry.project(&backends);
    let (other, selected) = setup(
        Script::pages([(
            None,
            Ok(Page {
                records: vec![item("42", "edited in tracker")?],
                next: None,
            }),
        )]),
        "example",
    )?;
    let newer_service = other.project(&selected);
    let (first, second) = tokio::join!(
        service.issues(&cache, project(), ReadMode::Refresh),
        async {
            adapter.started.notified().await;
            let latest = newer_service
                .issues(&cache, project(), ReadMode::Refresh)
                .await;
            adapter.finish.notify_one();
            latest
        }
    );
    let second_read = second?;
    let first_read = first?;
    assert_eq!(first_read.records, second_read.records);
    let disk = service
        .issue(&cache, lookup("42")?, ReadMode::Cached)
        .await?;
    assert!(
        matches!(disk.record.map(|o| o.state), Some(home::issue::IssueState::Invalid(record)) if record.title.as_deref()==Some("edited in tracker"))
    );
    Ok(())
}

#[tokio::test]
async fn a_looping_or_misrouted_first_scan_does_not_publish_any_records()
-> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let flow =
        home::flow::load(include_str!("../../home/src/flow/test-flow.toml"))?;
    let cache = IssueCache::new(dir.path(), &flow);
    let adapter = Script::pages([
        (
            None,
            Ok(Page {
                records: vec![item("1", "one")?],
                next: Some("second".into()),
            }),
        ),
        (
            Some("second"),
            Ok(Page {
                records: Vec::new(),
                next: Some("second".into()),
            }),
        ),
        (
            None,
            Ok(Page {
                records: vec![item("1", "wrong project")?],
                next: None,
            }),
        ),
    ]);
    let (registry, backends) = setup(adapter, "example")?;
    let service = registry.project(&backends);
    for selected in [project(), ProjectId::new(49)] {
        let read = service.issues(&cache, selected, ReadMode::Refresh).await?;
        assert_eq!(read.status.failure, Some(ReadFailure::InvalidResponse));
        assert!(!read.status.complete);
        assert_eq!(read.records, []);
        let offline =
            service.issues(&cache, selected, ReadMode::Cached).await?;
        assert_eq!(offline.status.freshness, Freshness::Unknown);
    }
    Ok(())
}

#[tokio::test]
async fn project_findings_require_observed_relations_and_visible_targets()
-> Result<(), Box<dyn Error>> {
    use home::issue::{Coverage, Issue, body::Body};
    let dir = tempfile::tempdir()?;
    let flow =
        home::flow::load(include_str!("../../home/src/flow/test-flow.toml"))?;
    let cache = IssueCache::new(dir.path(), &flow);
    let external = IssueId::new("elsewhere").ok_or("bad identity")?;
    let subject = Issue {
        id: IssueId::new("decision").ok_or("bad identity")?,
        project: project(),
        title: "Settled decision".into(),
        created: Some(UnixMillis(999)),
        kind: "decision".into(),
        status: "done".into(),
        stage: None,
        fields: BTreeMap::new(),
        links: BTreeMap::from([("blocked_by".into(), vec![external])]),
        body: String::new(),
        sections: vec![],
        checklist: vec![],
        log: vec![],
        notes: vec![],
        canvases: vec![],
        branch: "decision".into(),
    };
    let mut record = Observation::mapped(subject, Body::of("human text"));
    record.completeness.relations.insert(
        "parent".into(),
        Coverage::Incomplete {
            reason: "not fetched".into(),
        },
    );
    let adapter = Script::pages([(
        None,
        Ok(Page {
            records: vec![record],
            next: None,
        }),
    )]);
    let (registry, backends) = setup(adapter, "example")?;
    let list = registry
        .project(&backends)
        .issues(&cache, project(), ReadMode::Refresh)
        .await?;
    let observed = list.records.first().ok_or("missing decision")?;
    let messages = observed
        .flow_findings
        .iter()
        .map(|finding| finding.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        messages.contains("A settled decision records its answer"),
        "{messages}"
    );
    assert!(
        !messages.contains("Decisions and reviews belong to a spec"),
        "{messages}"
    );
    assert!(!messages.contains("is no issue"), "{messages}");
    Ok(())
}

#[tokio::test]
async fn two_projects_each_hold_their_own_issue_146_in_one_cache()
-> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let flow =
        home::flow::load(include_str!("../../home/src/flow/test-flow.toml"))?;
    let cache = IssueCache::new(dir.path(), &flow);
    let other = ProjectId::new(33);
    let elsewhere = Observation::invalid(Unreadable {
        id: IssueId::new("146").ok_or("fixture identity")?,
        project: other,
        title: Some("in the other project".into()),
        body: None,
        created: Some(UnixMillis(10)),
        reason: "unmapped label".into(),
    });
    let (our_registry, ours_backends) = setup(
        Script::pages([(
            None,
            Ok(Page {
                records: vec![item("146", "in our project")?],
                next: None,
            }),
        )]),
        "example",
    )?;
    let (their_registry, theirs_backends) = setup(
        Script::pages([(
            None,
            Ok(Page {
                records: vec![elsewhere],
                next: None,
            }),
        )]),
        "example",
    )?;
    let ours = our_registry.project(&ours_backends);
    let theirs = their_registry.project(&theirs_backends);
    ours.issues(&cache, project(), ReadMode::Refresh).await?;
    theirs.issues(&cache, other, ReadMode::Refresh).await?;
    let title = |reading: home::issue::IssueReading| {
        reading.record.and_then(|record| match record.state {
            home::issue::IssueState::Invalid(unread) => unread.title,
            _ => None,
        })
    };
    let id = IssueId::new("146").ok_or("fixture identity")?;
    let our_lookup = Lookup {
        project: project(),
        id: id.clone(),
    };
    let their_lookup = Lookup { project: other, id };
    assert_eq!(
        title(ours.issue(&cache, our_lookup, ReadMode::Cached).await?),
        Some(String::from("in our project"))
    );
    assert_eq!(
        title(theirs.issue(&cache, their_lookup, ReadMode::Cached).await?),
        Some(String::from("in the other project"))
    );
    Ok(())
}
