use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use home::ProjectId;
use home::backend::{Capability, CredentialState};
use home::issue::{
    ListPage, Locator, Lookup, Observation, Page, ReadFailure, Unreadable,
};
use home::{IssueId, UnixMillis};
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

struct Github(Observation);

impl IssueTracker for Github {
    fn read(&self, _: Lookup) -> IssueCompletion<'_, Observation> {
        Box::pin(async { Err(ReadFailure::Unavailable) })
    }

    fn list(&self, _: ListPage) -> IssueCompletion<'_, Page> {
        let page = Page {
            records: Vec::from([self.0.clone()]),
            next: None,
        };
        Box::pin(async move { Ok(page) })
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
}

fn project(root: &Path, id: &str, text: &str) -> std::io::Result<()> {
    let folder = root.join("projects").join(id);
    fs::create_dir_all(folder.join("issues"))?;
    fs::write(folder.join("project.toml"), text)
}

fn tracked(project: ProjectId) -> Result<Observation, Box<dyn Error>> {
    let mut record = Observation::invalid(Unreadable {
        id: IssueId::new("146").ok_or("id")?,
        project,
        title: Some(String::from("Kept by GitHub")),
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
    let loaded = home_files::read(root);
    let markdown = ProjectId::parse(MARKDOWN).ok_or("project")?;
    let github = ProjectId::parse(TRACKED).ok_or("project")?;
    let selection = loaded
        .snapshot
        .projects()
        .iter()
        .find(|known| known.id == github)
        .and_then(|known| known.backends.tracker.clone())
        .ok_or("tracker")?;
    let mut trackers = Trackers::new(root, loaded, Some(root.join("data")));
    trackers
        .registry
        .register_tracker(
            Registration {
                selection,
                capabilities: Vec::from([Capability::ReadIssues]),
                credential: CredentialState::NotRequired,
            },
            Arc::new(Github(tracked(github)?)),
        )
        .map_err(|refusal| refusal.label())?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut listed = |project| {
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
