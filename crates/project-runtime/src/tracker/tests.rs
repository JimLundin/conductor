use std::path::{Path, PathBuf};
use std::sync::Arc;

use home::canvas::{
    Block, Canvas, CanvasFile, CanvasKey, CanvasName, Stray, StrayReason,
};
use home::flow::snapshot::{Inputs, Snapshot};
use home::issue::ReadFailure;
use home::project::Project;
use home::{IssueId, ProjectId, UnixMillis};
use home_files::FolderStore;
use home_store::{CanvasStore, Memory, StoreError};

use super::{Connection, Connector, Reader, Trackers};

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
        issue: IssueId::new(issue).ok_or("issue")?,
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
        id: IssueId::new("146").ok_or("issue")?,
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
