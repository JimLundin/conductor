use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use home::UnixMillis;
use home::backend::{Capability, CredentialState, Selection};
use home::flow::resolve::Mode;
use home::flow::snapshot::Snapshot;
use home::issue::{
    ListPage, Locator, Lookup, Observation, Page, ReadFailure, Unreadable,
};
use home::project::Project;
use home::{IssueKey, ProjectId};
use home_store::Memory;
use project_runtime::tracker::{Connection, Connector, Reader};
use project_runtime::{IssueCompletion, IssueTracker, Registration};

use super::Trackers;

const FLOW: &str = include_str!("../../../home/src/flow/test-flow.toml");
const MARKDOWN: &str = "7w85je";
const TRACKED: &str = "hyn162";

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _removed = fs::remove_dir_all(&self.0);
    }
}

struct Github {
    read: Result<Observation, ReadFailure>,
    listed: Result<Observation, ReadFailure>,
}

impl IssueTracker for Github {
    fn native(
        &self,
        _: home::issue::Lookup,
    ) -> project_runtime::IssueCompletion<'_, home::issue::NativeIssue> {
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
    ) -> project_runtime::SendCompletion<'_, ()> {
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
    ) -> project_runtime::SendCompletion<'_, home::IssueId> {
        Box::pin(async { Err(home::issue::SendFailure::Unknown) })
    }
    fn opened(
        &self,
        _: home::ProjectId,
        _: home::operations::Handle,
    ) -> project_runtime::IssueCompletion<'_, Vec<home::issue::Opened>> {
        Box::pin(async { Err(home::issue::ReadFailure::Unavailable) })
    }
    fn logged(
        &self,
        _: &home::issue::Entry,
        _: home::operations::Handle,
    ) -> String {
        String::new()
    }
    fn read(&self, _: Lookup) -> IssueCompletion<'_, Observation> {
        let read = self.read.clone();
        Box::pin(async move { read })
    }

    fn list(&self, _: ListPage) -> IssueCompletion<'_, Page> {
        let page = self.listed.clone().map(|record| Page {
            records: Vec::from([record]),
            next: None,
        });
        Box::pin(async move { page })
    }
    fn comments(
        &self,
        _: home::issue::Lookup,
    ) -> project_runtime::IssueCompletion<'_, Vec<home::issue::Comment>> {
        Box::pin(async { Err(home::issue::ReadFailure::Unavailable) })
    }
    fn write_comment(
        &self,
        _: home::issue::Lookup,
        _: home::issue::CommentWrite,
    ) -> project_runtime::SendCompletion<'_, home::issue::Comment> {
        Box::pin(async { Err(home::issue::SendFailure::Unknown) })
    }
    fn plan_relation(
        &self,
        _: home::issue::Lookup,
        _: home::issue::RelationChange,
    ) -> project_runtime::RelationCompletion<'_> {
        Box::pin(async {
            Err(home::issue::Unrelatable::Read(
                home::issue::ReadFailure::Unavailable,
            ))
        })
    }
    fn write_relation(
        &self,
        _: home::issue::Lookup,
        _: home::issue::RelationStep,
    ) -> project_runtime::SendCompletion<'_, ()> {
        Box::pin(async { Err(home::issue::SendFailure::Unknown) })
    }
}

struct Scripted {
    selection: Selection,
    project: ProjectId,
    lists: bool,
}

impl Connector for Scripted {
    fn connect(&self) -> Result<Reader, ReadFailure> {
        Ok(Reader {
            registration: Registration {
                selection: self.selection.clone(),
                capabilities: Vec::from([Capability::ReadIssues]),
                credential: CredentialState::NotRequired,
            },
            tracker: Arc::new(if self.lists {
                Github {
                    read: Err(ReadFailure::Unavailable),
                    listed: tracked(self.project, "Kept by GitHub"),
                }
            } else {
                Github {
                    read: tracked(self.project, "Read just now"),
                    listed: Err(ReadFailure::RateLimited),
                }
            }),
        })
    }
}

fn connection(project: &Project, lists: bool) -> Result<Connection, String> {
    Ok(Connection {
        canvases: Box::new(Memory::default()),
        connector: Arc::new(Scripted {
            selection: project.backends.tracker.clone().ok_or("no tracker")?,
            project: project.id,
            lists,
        }),
    })
}

fn scripted(
    _: &Path,
    _: &Snapshot,
    project: &Project,
) -> Result<Connection, String> {
    connection(project, true)
}

fn reading_only(
    _: &Path,
    _: &Snapshot,
    project: &Project,
) -> Result<Connection, String> {
    connection(project, false)
}

fn project(root: &Path, id: &str, text: &str) -> std::io::Result<()> {
    let folder = root.join("projects").join(id);
    fs::create_dir_all(folder.join("issues"))?;
    fs::write(folder.join("project.toml"), text)
}

fn tracked(
    project: ProjectId,
    title: &str,
) -> Result<Observation, ReadFailure> {
    let mut record = Observation::invalid(Unreadable {
        id: github::ISSUER
            .read("146")
            .ok_or(ReadFailure::InvalidResponse)?,
        project,
        title: Some(String::from(title)),
        body: Some(String::from("human text")),
        created: Some(UnixMillis(10)),
        reason: String::from("unmapped label"),
    });
    record.locator = Some(Locator {
        repository: String::from("owner/standards"),
        url: None,
        addresses: BTreeMap::new(),
    });
    Ok(record)
}

fn shape(json: &serde_json::Value) -> Vec<Vec<String>> {
    let keys = |value: &serde_json::Value| {
        value
            .as_object()
            .map_or_else(Vec::new, |object| object.keys().cloned().collect())
    };
    Vec::from([keys(json), keys(&json["status"])])
}

fn scratch(name: &str) -> Result<Scratch, Box<dyn Error>> {
    let scratch = Scratch(
        std::env::temp_dir()
            .join(format!("home-{name}-{}", std::process::id())),
    );
    let root = &scratch.0;
    fs::create_dir_all(root.join("docs/agents"))?;
    fs::write(root.join("docs/agents/flow.toml"), FLOW)?;
    project(
        root,
        TRACKED,
        "name = \"standards\"\ntitle = \"Standards\"\nroot = \"e22b7f7b2976650400965d4147ba771675f96076\"\n[backends.tracker]\nadapter = \"github\"\nresource = \"owner/standards\"\n",
    )?;
    Ok(scratch)
}

#[test]
fn a_fresh_issue_show_of_a_cached_issue_reads_it_and_never_lists_its_tracker()
-> Result<(), Box<dyn Error>> {
    let scratch = scratch("show-fresh")?;
    let root = &scratch.0;
    let github = ProjectId::parse(TRACKED).ok_or("project")?;
    let data = Some(root.join("data"));
    let listing = Trackers::new(
        root,
        home_files::read(root, github::ISSUERS),
        data.clone(),
        scripted,
    )?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(listing.listing(github, false))?;
    let trackers = Trackers::new(
        root,
        home_files::read(root, github::ISSUERS),
        data,
        reading_only,
    )?;
    let key = IssueKey {
        project: github,
        id: github::ISSUER.read("146").ok_or("id")?,
    };
    let resolved =
        trackers.resolved(BTreeSet::from([key.clone()]), Mode::Fresh);
    let reading = resolved.get(&key).ok_or("unresolved")?.clone()?;
    let shown = serde_json::to_value(&reading)?;
    assert_eq!(
        (
            &shown["record"]["state"]["Invalid"]["title"],
            &shown["status"]["failure"]
        ),
        (
            &serde_json::json!("Read just now"),
            &serde_json::Value::Null
        )
    );
    Ok(())
}

#[test]
fn issue_list_lists_a_markdown_project_and_a_tracker_project_in_one_shape()
-> Result<(), Box<dyn Error>> {
    let scratch = Scratch(
        std::env::temp_dir()
            .join(format!("home-list-trackers-{}", std::process::id())),
    );
    let root = &scratch.0;
    fs::create_dir_all(root.join("docs/agents"))?;
    fs::write(root.join("docs/agents/flow.toml"), FLOW)?;
    project(
        root,
        MARKDOWN,
        "name = \"home\"\ntitle = \"Home\"\nroot = \"fc27e50efb661d7ef4a45d3edbe0234ff0a6fbd5\"\n",
    )?;
    fs::write(
        root.join("projects")
            .join(MARKDOWN)
            .join("issues/0tm7aaaa.md"),
        "+++\nid = \"0tm7aaaa\"\ntitle = \"Kept in the home\"\nkind = \"bug\"\nstatus = \"open\"\ncreated = 2026-11-25T09:21:48Z\n+++\n",
    )?;
    project(
        root,
        TRACKED,
        "name = \"standards\"\ntitle = \"Standards\"\nroot = \"e22b7f7b2976650400965d4147ba771675f96076\"\n[backends.tracker]\nadapter = \"github\"\nresource = \"owner/standards\"\n",
    )?;
    let loaded = home_files::read(root, github::ISSUERS);
    let markdown = ProjectId::parse(MARKDOWN).ok_or("project")?;
    let github = ProjectId::parse(TRACKED).ok_or("project")?;
    let trackers =
        Trackers::new(root, loaded, Some(root.join("data")), scripted)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let listed = |project| {
        runtime
            .block_on(trackers.listing(project, false))
            .and_then(|listing| {
                let ids: Vec<String> = listing
                    .records
                    .iter()
                    .map(|record| record.id().to_string())
                    .collect();
                let (said, code) = super::super::tracking::shown(&listing)?;
                let json: serde_json::Value = serde_json::from_str(&said)
                    .map_err(|error| error.to_string())?;
                Ok((ids, shape(&json), code))
            })
    };
    let home = listed(markdown)?;
    let tracker = listed(github)?;
    assert_eq!(home.0, Vec::from([String::from("0tm7aaaa")]));
    assert_eq!(tracker.0, Vec::from([String::from("146")]));
    assert_eq!(home.1, tracker.1);
    assert_eq!((home.2, tracker.2), (0, 0));
    Ok(())
}

#[test]
fn a_canvas_put_on_a_tracked_project_needs_no_repository()
-> Result<(), Box<dyn Error>> {
    let scratch = scratch("tracked-put")?;
    let root = &scratch.0;
    let canvas = root.join("design.json");
    fs::write(
        &canvas,
        "{\"title\":\"Kept by its tracker\",\"blocks\":[{\"prose\":{\"text\":\"The tracker keeps it.\"}}]}",
    )?;
    let file = canvas.to_string_lossy().into_owned();
    let args: Vec<String> = [
        "canvas",
        "put",
        "standards",
        "146",
        "design",
        "--file",
        &file,
    ]
    .map(String::from)
    .into();
    let loaded = home_files::read(root, github::ISSUERS);
    let context = crate::context();
    let fresh = home::issue::storage::fresh(
        &loaded.snapshot,
        context.now.secs(),
        &[7; home::DRAWS],
    );
    let home::flow::command::Asked::Flow(command) =
        home::flow::command::asked(&args, &loaded.snapshot, fresh)
    else {
        return Err("canvas put isn't a flow command".into());
    };
    let mut trackers =
        Trackers::new(root, loaded, Some(root.join("data")), scripted)?;
    let said = crate::issues(root, &mut trackers, command, &context);
    assert_eq!(
        said,
        (String::from("wrote canvas design of standards/146\n"), 0)
    );
    Ok(())
}
