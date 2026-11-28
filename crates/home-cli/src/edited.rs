use std::collections::BTreeMap;
use std::error::Error;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use github::configuration::Configuration;
use github::mapping::Record;
use home::backend::{Capability, CredentialState};
use home::flow::Flow;
use home::flow::command::{Asked, Mutation, asked};
use home::flow::snapshot::Snapshot;
use home::issue::storage::fresh;
use home::issue::{
    Comment, CommentWrite, IssueWrite, ListPage, Locator, Lookup, NativeIssue,
    Observation, Page, ReadFailure, SendFailure,
};
use home::project::Project;
use home::{IssueId, ProjectId, UnixMillis};
use home_store::Memory;
use project_runtime::operations::Operations;
use project_runtime::tracker::{Connection, Connector, Reader};
use project_runtime::{
    IssueCompletion, IssueTracker, Registration, SendCompletion,
};

use super::resolve::Trackers;

const FLOW: &str = include_str!("../../home/src/flow/test-flow.toml");
const FOOTER: &str =
    "```project-data\nschema = 1\n\n[fields]\nbranch = \"kept\"\n```\n";

struct Github {
    flow: Flow,
    config: Configuration,
    project: ProjectId,
    id: IssueId,
    data: PathBuf,
}

const ISSUE: &str = "scripted-issue.json";
const REQUESTS: &str = "scripted-requests";

fn kept(data: &Path) -> Result<NativeIssue, ReadFailure> {
    std::fs::read(data.join(ISSUE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or(ReadFailure::Unavailable)
}

fn keep(data: &Path, issue: &NativeIssue) -> Result<(), String> {
    serde_json::to_vec(issue)
        .map_err(|error| error.to_string())
        .and_then(|bytes| {
            std::fs::write(data.join(ISSUE), bytes)
                .map_err(|error| error.to_string())
        })
}

impl Github {
    fn asked(&self, request: &str) {
        let _logged = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.data.join(REQUESTS))
            .and_then(|mut log| writeln!(log, "{request}"));
    }

    fn observed(&self) -> Result<Observation, ReadFailure> {
        Ok(github::mapping::observe(
            &self.flow,
            &self.config,
            self.project,
            Record {
                id: self.id.clone(),
                locator: Locator {
                    repository: String::from("owner/standards"),
                    url: None,
                    addresses: BTreeMap::new(),
                },
                created: UnixMillis(1000),
                native: kept(&self.data)?,
            },
        ))
    }
}

fn applied(mut issue: NativeIssue, write: &IssueWrite) -> NativeIssue {
    match write {
        IssueWrite::AddLabel(name) => issue.labels.push(name.clone()),
        IssueWrite::RemoveLabel(name) => {
            issue.labels.retain(|label| label != name);
        }
        IssueWrite::Title { to, .. } => issue.title.clone_from(to),
        IssueWrite::Body { to, .. } => issue.body.clone_from(to),
        IssueWrite::Close { reason, .. } => {
            issue.open = false;
            issue.reason = Some(reason.clone());
        }
        IssueWrite::Reopen { .. } => {
            issue.open = true;
            issue.reason = Some(String::from("reopened"));
        }
    }
    issue
}

impl IssueTracker for Github {
    fn read(&self, _: Lookup) -> IssueCompletion<'_, Observation> {
        self.asked("read");
        let observed = self.observed();
        Box::pin(async move { observed })
    }

    fn list(&self, _: ListPage) -> IssueCompletion<'_, Page> {
        self.asked("list");
        let page = self.observed().map(|observed| Page {
            records: Vec::from([observed]),
            next: None,
        });
        Box::pin(async move { page })
    }

    fn comments(&self, _: Lookup) -> IssueCompletion<'_, Vec<Comment>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn write_comment(
        &self,
        _: Lookup,
        _: CommentWrite,
    ) -> SendCompletion<'_, Comment> {
        Box::pin(async { Err(SendFailure::Unknown) })
    }

    fn native(&self, _: Lookup) -> IssueCompletion<'_, NativeIssue> {
        self.asked("native");
        let native = kept(&self.data);
        Box::pin(async move { native })
    }

    fn writes(
        &self,
        observed: &NativeIssue,
        mutations: &[Mutation],
    ) -> Result<Vec<IssueWrite>, String> {
        github::mapping::writes(&self.flow, &self.config, observed, mutations)
    }

    fn write_issue(
        &self,
        _: Lookup,
        write: IssueWrite,
    ) -> SendCompletion<'_, ()> {
        self.asked(&format!("{write:?}"));
        let written =
            kept(&self.data)
                .map_err(SendFailure::Rejected)
                .and_then(|issue| {
                    keep(&self.data, &applied(issue, &write))
                        .map_err(|_| SendFailure::Unknown)
                });
        Box::pin(async move { written })
    }
}

struct Connected(Arc<Github>, home::backend::Selection);

impl Connector for Connected {
    fn connect(&self) -> Result<Reader, ReadFailure> {
        self.0.asked("connect");
        Ok(Reader {
            registration: Registration {
                selection: self.1.clone(),
                capabilities: Vec::from([
                    Capability::ReadIssues,
                    Capability::WriteIssues,
                ]),
                credential: CredentialState::NotRequired,
            },
            tracker: Arc::clone(&self.0) as Arc<dyn IssueTracker>,
        })
    }
}

fn connect(
    data: &Path,
    _: &Snapshot,
    project: &Project,
) -> Result<Connection, String> {
    let flow = home::flow::load(FLOW)?;
    let config = github::configuration::load("", &flow)?;
    let github = Arc::new(Github {
        flow,
        config,
        project: project.id,
        id: IssueId::new("146").ok_or("id")?,
        data: data.to_path_buf(),
    });
    Ok(Connection {
        canvases: Box::new(Memory::default()),
        connector: Arc::new(Connected(
            github,
            project.backends.tracker.clone().ok_or("no tracker")?,
        )),
    })
}

struct Home {
    root: PathBuf,
}

impl Drop for Home {
    fn drop(&mut self) {
        let _removed = std::fs::remove_dir_all(&self.root);
    }
}

impl Home {
    fn new(
        name: &str,
        labels: &[&str],
        body: &str,
    ) -> Result<Self, Box<dyn Error>> {
        let root = std::env::temp_dir()
            .join(format!("home-edits-{name}-{}", std::process::id()));
        let project = root.join("projects/hyn162");
        std::fs::create_dir_all(&project)?;
        std::fs::create_dir_all(root.join("docs/agents"))?;
        std::fs::write(root.join("docs/agents/flow.toml"), FLOW)?;
        std::fs::write(
            project.join("project.toml"),
            "name = \"standards\"\ntitle = \"Standards\"\nroot = \"e22b7f7b2976650400965d4147ba771675f96076\"\n[backends.tracker]\nadapter = \"github\"\nresource = \"owner/standards\"\n",
        )?;
        let data = root.join("data");
        Operations::bind(&data, [7; 32])?;
        keep(
            &data,
            &NativeIssue {
                title: String::from("Kept by GitHub"),
                body: String::from(body),
                labels: labels.iter().copied().map(String::from).collect(),
                open: true,
                reason: None,
            },
        )?;
        Ok(Self { root })
    }

    fn home(&self, words: &[&str]) -> Result<(String, u8), Box<dyn Error>> {
        let args: Vec<String> =
            words.iter().copied().map(String::from).collect();
        let loaded = home_files::read(&self.root);
        let context = super::context();
        let fresh =
            fresh(&loaded.snapshot, context.now.secs(), &[7; home::DRAWS]);
        let Asked::Flow(command) = asked(&args, &loaded.snapshot, fresh) else {
            return Err("not a flow command".into());
        };
        let mut trackers = Trackers::new(
            &self.root,
            loaded,
            Some(self.root.join("data")),
            connect,
        )?;
        Ok(super::issues(&self.root, &mut trackers, command, &context))
    }

    fn requests(&self) -> Vec<String> {
        let path = self.root.join("data").join(REQUESTS);
        let said = std::fs::read_to_string(&path).unwrap_or_default();
        let _cleared = std::fs::remove_file(&path);
        said.lines().map(String::from).collect()
    }

    fn issue(&self) -> Result<NativeIssue, ReadFailure> {
        kept(&self.root.join("data"))
    }
}

#[test]
fn issue_move_stage_spec_on_a_github_issue_swaps_its_stage_label_and_nothing_else()
-> Result<(), Box<dyn Error>> {
    let home =
        Home::new("move", &["Customer report", "work/stage/shape"], "Words\n")?;
    let (said, code) =
        home.home(&["issue", "move", "standards", "146", "stage", "spec"])?;
    assert!(said.starts_with("wrote standards/146\n"), "{said}");
    assert_eq!(code, 0, "{said}");
    let writes: Vec<String> = home
        .requests()
        .into_iter()
        .filter(|request| {
            !["connect", "list", "read", "native"].contains(&request.as_str())
        })
        .collect();
    assert_eq!(
        writes,
        [
            "AddLabel(\"work/stage/spec\")",
            "RemoveLabel(\"work/stage/shape\")"
        ]
    );
    let issue = home.issue()?;
    assert_eq!(issue.labels, ["Customer report", "work/stage/spec"]);
    assert_eq!(issue.body, "Words\n");
    assert!(!home.root.join("projects/hyn162/issues").exists());
    Ok(())
}

#[test]
fn a_move_into_a_sign_off_stage_without_signing_off_is_refused_before_any_request()
-> Result<(), Box<dyn Error>> {
    let home = Home::new("sign-off", &["work/stage/spec"], "Words\n")?;
    home.home(&["issue", "show", "standards", "146"])?;
    home.requests();
    let (said, code) =
        home.home(&["issue", "move", "standards", "146", "stage", "slice"])?;
    assert!(said.contains("needs its sign-off"), "{said}");
    assert_eq!(code, 1);
    assert_eq!(home.requests(), Vec::<String>::new());
    assert_eq!(home.issue()?.labels, ["work/stage/spec"]);
    Ok(())
}

#[test]
fn issue_body_on_a_github_issue_keeps_its_project_data_footer()
-> Result<(), Box<dyn Error>> {
    let home = Home::new("body", &[], &format!("Old words\n\n{FOOTER}"))?;
    let (said, code) =
        home.home(&["issue", "body", "standards", "146", "New", "words"])?;
    assert!(said.starts_with("wrote standards/146\n"), "{said}");
    assert_eq!(code, 0, "{said}");
    assert_eq!(home.issue()?.body, format!("New words\n\n{FOOTER}"));
    Ok(())
}
