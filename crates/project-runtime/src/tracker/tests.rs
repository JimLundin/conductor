use std::path::{Path, PathBuf};
use std::sync::Arc;

use home::backend::{Capability, CredentialState};
use home::canvas::{
    Block, Canvas, CanvasFile, CanvasKey, CanvasName, Stray, StrayReason,
};
use home::canvas::{Broken, CanvasEntry, Held};
use home::flow::snapshot::{Inputs, Snapshot};
use home::issue::{
    Comment, CommentWrite, IssueListing, ListPage, Lookup, Observation, Page,
    ReadFailure, SendFailure,
};
use home::project::Project;
use home::{IssueId, IssueKey, ProjectId, UnixMillis};
use home_files::FolderStore;
use home_store::{CanvasStore, Memory, Putting, StoreError};

use super::{
    Canvases, Connection, Connector, Reader, Refreshing, TrackerStore, Trackers,
};
use crate::{IssueCompletion, IssueTracker, Registration, SendCompletion};

const FLOW: &str = include_str!("../../../home/src/flow/test-flow.toml");
const MARKDOWN: &str = "7w85je";
const TRACKED: &str = "hyn162";

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Result<Self, String> {
        let path = std::env::temp_dir()
            .join(format!("tracker-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&path).map_err(|error| error.to_string())?;
        Ok(Self(path))
    }

    fn files(&self) -> Result<usize, String> {
        std::fs::read_dir(&self.0)
            .map(Iterator::count)
            .map_err(|error| error.to_string())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _removed = std::fs::remove_dir_all(&self.0);
    }
}

struct Offline;

impl Connector for Offline {
    fn connect(&self) -> Result<Reader, ReadFailure> {
        Err(ReadFailure::Unavailable)
    }
}

fn home() -> Snapshot {
    let project = |name: &str, tracker: &str| {
        Ok(format!(
            "name = \"{name}\"\ntitle = \"{name}\"\nroot = \"e22b7f7b2976650400965d4147ba771675f96076\"\n{tracker}"
        ))
    };
    Snapshot::of(Inputs {
        flow: home::flow::load(FLOW),
        projects: home::project::listed(Vec::from([
            (String::from(MARKDOWN), project("home", "")),
            (
                String::from(TRACKED),
                project(
                    "standards",
                    "[backends.tracker]\nadapter = \"github\"\nresource = \"owner/standards\"\n",
                ),
            ),
        ])),
        issues: Vec::new(),
        unread: Vec::new(),
        issuers: home::tracker::Issuers::new(&[github::ISSUER]),
    })
}

fn id(project: &str) -> Result<ProjectId, String> {
    ProjectId::parse(project).ok_or_else(|| String::from("project"))
}

fn file() -> Result<CanvasFile, String> {
    Ok(CanvasFile {
        written: UnixMillis(5),
        by: String::from("devbox·claude"),
        canvas: Canvas::new(
            String::from("Kept by its tracker"),
            Vec::from([Block::Prose {
                text: String::from("The tracker keeps it."),
            }]),
        )
        .map_err(|error| error.to_string())?,
    })
}

fn key(project: &str, issue: &str) -> Result<CanvasKey, String> {
    Ok(CanvasKey {
        project: id(project)?,
        issue: if project == MARKDOWN {
            home::tracker::Issuer::MARKDOWN
        } else {
            github::ISSUER
        }
        .read(issue)
        .ok_or("issue")?,
        name: CanvasName::parse("design").map_err(|error| error.to_string())?,
    })
}

fn stray(place: &str) -> Stray {
    Stray {
        place: String::from(place),
        reason: StrayReason::Unnamed,
    }
}

fn unmapped(_: &Path, _: &Snapshot, _: &Project) -> Result<Connection, String> {
    Err(String::from("unknown field `colour`"))
}

fn followed(
    home_store: Box<dyn CanvasStore + Send + Sync>,
    connect: super::Connect,
) -> Trackers {
    let mut trackers =
        Trackers::new(PathBuf::from("data"), home_store, connect);
    trackers.follow(&home());
    trackers
}

#[tokio::test]
async fn a_tracked_project_s_canvases_are_kept_by_its_tracker_s_store()
-> Result<(), String> {
    let scratch = Scratch::new("kept")?;
    let mut trackers =
        followed(Box::new(FolderStore::new(scratch.0.clone())), |_, _, _| {
            Ok(Connection {
                canvases: Box::new(Memory::default()),
                connector: Arc::new(Offline),
            })
        });
    let held = key(TRACKED, "146")?;
    let homed = key(MARKDOWN, "0tm7aaaa")?;
    let wrote = (
        trackers
            .put(&held, &file()?)
            .await
            .map_err(|error| error.to_string())?,
        trackers
            .put(&homed, &file()?)
            .await
            .map_err(|error| error.to_string())?,
    );
    assert_eq!(
        (
            wrote.0.home_file,
            wrote.1.home_file,
            trackers.get(&held).map_err(|error| error.to_string())?,
            trackers.all().map_err(|error| error.to_string())?.canvases,
        ),
        (
            None,
            Some(String::from("projects/7w85je/issues/0tm7aaaa/design.json")),
            Some(file()?),
            Vec::from([homed, held]),
        )
    );
    Ok(())
}

#[tokio::test]
async fn a_put_on_a_tracked_project_with_no_store_is_unavailable_and_writes_nothing_to_the_home()
-> Result<(), String> {
    let scratch = Scratch::new("unmapped")?;
    let mut trackers =
        followed(Box::new(FolderStore::new(scratch.0.clone())), unmapped);
    let put = trackers.put(&key(TRACKED, "146")?, &file()?).await;
    assert_eq!(
        put,
        Err(StoreError::Unavailable {
            reason: String::from(
                "the tracker of standards: unknown field `colour`"
            ),
        })
    );
    assert_eq!(scratch.files()?, 0);
    Ok(())
}

#[tokio::test]
async fn a_tracked_project_whose_store_cant_be_built_fails_its_lookups_naming_why()
-> Result<(), String> {
    let trackers = followed(Box::new(Memory::default()), unmapped);
    let issue = home::IssueKey {
        project: id(TRACKED)?,
        id: github::ISSUER.read("146").ok_or("issue")?,
    };
    let reason =
        String::from("the tracker of standards: unknown field `colour`");
    assert_eq!(
        (
            trackers.look(&home(), &issue).await,
            trackers
                .listing(&home(), issue.project, true)
                .await
                .map(drop),
            trackers.list(&issue.project, &issue.id).map(drop),
        ),
        (
            Err(reason.clone()),
            Err(reason.clone()),
            Err(StoreError::Unavailable { reason }),
        )
    );
    Ok(())
}

#[test]
fn all_reports_the_strays_of_every_tracker_s_store() -> Result<(), String> {
    let mut home_store = Memory::default();
    home_store.stray(id(MARKDOWN)?, None, stray("projects/7w85je/odd"));
    let trackers = followed(Box::new(home_store), |_, _, project| {
        let mut store = Memory::default();
        store.stray(project.id, None, stray("issues/146#issuecomment-1"));
        Ok(Connection {
            canvases: Box::new(store),
            connector: Arc::new(Offline),
        })
    });
    assert_eq!(
        trackers.all().map(|held| held.strays),
        Ok(Vec::from([
            stray("projects/7w85je/odd"),
            stray("issues/146#issuecomment-1"),
        ]))
    );
    Ok(())
}

#[test]
fn all_lists_the_built_projects_canvases_and_reports_an_unbuilt_project_on_its_own()
-> Result<(), String> {
    let mut home_store = Memory::default();
    let homed = key(MARKDOWN, "0tm7aaaa")?;
    home_store.keep(&homed, &file()?);
    let trackers = followed(Box::new(home_store), unmapped);
    assert_eq!(
        trackers.canvases(),
        Ok(Canvases {
            held: Held {
                canvases: Vec::from([homed]),
                strays: Vec::new(),
            },
            unavailable: Vec::from([String::from(
                "the tracker of standards: unknown field `colour`"
            )]),
        })
    );
    Ok(())
}

struct Unreachable;

impl Unreachable {
    fn failed<T>() -> Result<T, StoreError> {
        Err(StoreError::Unavailable {
            reason: String::from("GitHub is unreachable"),
        })
    }
}

impl CanvasStore for Unreachable {
    fn all(&self) -> Result<Held<CanvasKey>, StoreError> {
        Self::failed()
    }

    fn list(
        &self,
        _: &ProjectId,
        _: &IssueId,
    ) -> Result<Held<Result<CanvasEntry, Broken>>, StoreError> {
        Self::failed()
    }

    fn get(&self, _: &CanvasKey) -> Result<Option<CanvasFile>, StoreError> {
        Self::failed()
    }

    fn put<'a>(
        &'a mut self,
        _: &'a CanvasKey,
        _: &'a CanvasFile,
    ) -> Putting<'a> {
        Box::pin(async { Self::failed() })
    }
}

impl TrackerStore for Unreachable {
    fn refresh<'a>(&'a self, _: &'a IssueListing) -> Refreshing<'a> {
        Box::pin(async { Err(ReadFailure::Unavailable) })
    }

    fn refresh_issue<'a>(&'a self, _: &'a IssueKey) -> Refreshing<'a> {
        Box::pin(async { Err(ReadFailure::Unavailable) })
    }
}

#[test]
fn all_lists_the_markdown_canvases_and_reports_a_tracker_whose_store_fails_on_its_own()
-> Result<(), String> {
    let mut home_store = Memory::default();
    let homed = key(MARKDOWN, "0tm7aaaa")?;
    home_store.keep(&homed, &file()?);
    let trackers = followed(Box::new(home_store), |_, _, _| {
        Ok(Connection {
            canvases: Box::new(Unreachable),
            connector: Arc::new(Offline),
        })
    });
    assert_eq!(
        trackers.canvases(),
        Ok(Canvases {
            held: Held {
                canvases: Vec::from([homed]),
                strays: Vec::new(),
            },
            unavailable: Vec::from([String::from(
                "the tracker of standards: GitHub is unreachable"
            )]),
        })
    );
    Ok(())
}

struct Empty;

impl IssueTracker for Empty {
    fn native(
        &self,
        _: home::issue::Lookup,
    ) -> crate::IssueCompletion<'_, home::issue::NativeIssue> {
        Box::pin(async { Err(home::issue::ReadFailure::Unavailable) })
    }
    fn writes(
        &self,
        _: &Lookup,
        _: &home::issue::NativeIssue,
        _: &[home::flow::command::Mutation],
    ) -> Result<Vec<home::issue::IssueWrite>, String> {
        Err(String::from("the script takes no edits"))
    }
    fn write_issue(
        &self,
        _: home::issue::Lookup,
        _: home::issue::IssueWrite,
    ) -> crate::SendCompletion<'_, ()> {
        Box::pin(async { Err(home::issue::SendFailure::Unknown) })
    }
    fn opening(
        &self,
        _: &home::flow::command::Change,
        _: home::operations::Handle,
    ) -> Result<home::issue::IssueCreate, String> {
        Err(String::from("the script opens no issues"))
    }
    fn create(
        &self,
        _: home::ProjectId,
        _: home::issue::IssueCreate,
    ) -> crate::SendCompletion<'_, home::IssueId> {
        Box::pin(async { Err(home::issue::SendFailure::Unknown) })
    }
    fn opened(
        &self,
        _: home::ProjectId,
        _: home::operations::Handle,
    ) -> crate::IssueCompletion<'_, home::issue::Openings> {
        Box::pin(async { Err(home::issue::ReadFailure::Unavailable) })
    }
    fn logged(
        &self,
        _: &home::issue::Entry,
        _: home::operations::Handle,
    ) -> String {
        String::new()
    }
    fn read(&self, lookup: Lookup) -> IssueCompletion<'_, Observation> {
        Box::pin(async move {
            Ok(Observation::invalid(home::issue::Unreadable {
                id: lookup.id,
                project: lookup.project,
                title: None,
                body: None,
                created: None,
                reason: String::from("unmapped label"),
            }))
        })
    }

    fn list(&self, _: ListPage) -> IssueCompletion<'_, Page> {
        Box::pin(async {
            Ok(Page {
                records: Vec::new(),
                next: None,
            })
        })
    }

    fn comments(&self, _: Lookup) -> IssueCompletion<'_, Vec<Comment>> {
        Box::pin(async { Err(ReadFailure::Unavailable) })
    }

    fn write_comment(
        &self,
        _: Lookup,
        _: CommentWrite,
    ) -> SendCompletion<'_, Comment> {
        Box::pin(async { Err(SendFailure::Unknown) })
    }

    fn plan_relation(
        &self,
        _: Lookup,
        _: home::issue::RelationChange,
    ) -> crate::RelationCompletion<'_> {
        Box::pin(async {
            Err(home::issue::Unrelatable::Read(ReadFailure::Unavailable))
        })
    }

    fn write_relation(
        &self,
        _: Lookup,
        _: home::issue::RelationStep,
    ) -> SendCompletion<'_, ()> {
        Box::pin(async { Err(SendFailure::Unknown) })
    }
}

struct Online(home::backend::Selection);

impl Connector for Online {
    fn connect(&self) -> Result<Reader, ReadFailure> {
        Ok(Reader {
            registration: Registration {
                selection: self.0.clone(),
                capabilities: Vec::from([Capability::ReadIssues]),
                credential: CredentialState::NotRequired,
            },
            tracker: Arc::new(Empty),
        })
    }
}

struct RateLimited(Memory);

impl CanvasStore for RateLimited {
    fn all(&self) -> Result<Held<CanvasKey>, StoreError> {
        self.0.all()
    }

    fn list(
        &self,
        project: &ProjectId,
        issue: &IssueId,
    ) -> Result<Held<Result<CanvasEntry, Broken>>, StoreError> {
        self.0.list(project, issue)
    }

    fn get(&self, key: &CanvasKey) -> Result<Option<CanvasFile>, StoreError> {
        self.0.get(key)
    }

    fn put<'a>(
        &'a mut self,
        key: &'a CanvasKey,
        file: &'a CanvasFile,
    ) -> Putting<'a> {
        self.0.put(key, file)
    }
}

impl TrackerStore for RateLimited {
    fn refresh<'a>(&'a self, _: &'a IssueListing) -> Refreshing<'a> {
        Box::pin(async { Err(ReadFailure::RateLimited) })
    }

    fn refresh_issue<'a>(&'a self, _: &'a IssueKey) -> Refreshing<'a> {
        Box::pin(async { Err(ReadFailure::RateLimited) })
    }
}

#[tokio::test]
async fn a_refresh_of_a_tracked_project_refreshes_its_tracker_s_store_and_an_offline_listing_does_not()
-> Result<(), String> {
    let scratch = Scratch::new("refresh")?;
    let mut trackers = Trackers::new(
        scratch.0.clone(),
        Box::new(Memory::default()),
        |_, _, project| {
            Ok(Connection {
                canvases: Box::new(RateLimited(Memory::default())),
                connector: Arc::new(Online(
                    project.backends.tracker.clone().ok_or("no tracker")?,
                )),
            })
        },
    );
    trackers.follow(&home());
    let tracked = id(TRACKED)?;
    assert_eq!(
        (
            trackers.refresh(&home(), tracked).await?,
            trackers
                .listing(&home(), tracked, true)
                .await?
                .status
                .failure,
        ),
        (Err(ReadFailure::RateLimited), None)
    );
    Ok(())
}

#[tokio::test]
async fn a_fresh_read_of_a_tracked_issue_refreshes_its_canvases_and_an_offline_look_does_not()
-> Result<(), String> {
    let scratch = Scratch::new("refresh-issue")?;
    let mut trackers = Trackers::new(
        scratch.0.clone(),
        Box::new(Memory::default()),
        |_, _, project| {
            Ok(Connection {
                canvases: Box::new(RateLimited(Memory::default())),
                connector: Arc::new(Online(
                    project.backends.tracker.clone().ok_or("no tracker")?,
                )),
            })
        },
    );
    trackers.follow(&home());
    let issue = home::IssueKey {
        project: id(TRACKED)?,
        id: github::ISSUER.read("146").ok_or("issue")?,
    };
    assert_eq!(
        (
            trackers.read(&home(), &issue).await?.status.failure,
            trackers.look(&home(), &issue).await?.status.failure,
        ),
        (Some(ReadFailure::RateLimited), None)
    );
    Ok(())
}
