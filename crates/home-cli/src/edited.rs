use std::collections::BTreeMap;
use std::error::Error;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use github::configuration::Configuration;
use github::log::Posted;
use github::mapping::relate::{self, Request};
use github::mapping::{Native, Record};
use home::backend::{Capability, CredentialState};
use home::flow::Flow;
use home::flow::command::{Asked, Change, Mutation, asked};
use home::flow::snapshot::Snapshot;
use home::issue::storage::fresh;
use home::issue::{
    Comment, CommentWrite, Entry, IssueCreate, IssueWrite, ListPage, Locator,
    Lookup, NativeIssue, Observation, Opened, Page, ReadFailure, Relating,
    RelationChange, RelationStep, SendFailure, Unrelatable,
};
use home::operations::Handle;
use home::project::Project;
use home::tracker::Ids;
use home::{IssueId, ProjectId, UnixMillis};
use home_store::Memory;
use project_runtime::operations::Operations;
use project_runtime::tracker::{Connection, Connector, Reader};
use project_runtime::{
    IssueCompletion, IssueTracker, Registration, RelationCompletion,
    SendCompletion,
};

use super::resolve::Trackers;

const FLOW: &str = include_str!("../../home/src/flow/test-flow.toml");
const FOOTER: &str =
    "```project-data\nschema = 1\n\n[fields]\nbranch = \"kept\"\n```\n";

struct Github {
    flow: Flow,
    config: Configuration,
    project: ProjectId,
    data: PathBuf,
}

const ISSUES: &str = "scripted-issues.json";
const COMMENTS: &str = "scripted-comments.json";
const LOSE: &str = "scripted-lose";
const REQUESTS: &str = "scripted-requests";
const PARENTS: &str = "scripted-parents.json";

type Issues = BTreeMap<String, NativeIssue>;

fn issues(data: &Path) -> Result<Issues, ReadFailure> {
    std::fs::read(data.join(ISSUES))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or(ReadFailure::Unavailable)
}

fn kept(data: &Path, id: &str) -> Result<NativeIssue, ReadFailure> {
    issues(data)?.remove(id).ok_or(ReadFailure::Deleted)
}

fn keep(data: &Path, id: &str, issue: &NativeIssue) -> Result<(), String> {
    let mut all = issues(data).unwrap_or_default();
    all.insert(String::from(id), issue.clone());
    save(data, ISSUES, serde_json::to_vec(&all))
}

fn save(
    data: &Path,
    name: &str,
    encoded: serde_json::Result<Vec<u8>>,
) -> Result<(), String> {
    encoded
        .map_err(|error| error.to_string())
        .and_then(|bytes| {
            std::fs::write(data.join(name), bytes)
                .map_err(|error| error.to_string())
        })
}

fn comments(data: &Path) -> Vec<(String, Comment)> {
    std::fs::read(data.join(COMMENTS))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn on(data: &Path, issue: &IssueId) -> Vec<Comment> {
    comments(data)
        .into_iter()
        .filter(|(on, _)| *on == issue.to_string())
        .map(|(_, comment)| comment)
        .collect()
}

fn parents(data: &Path) -> BTreeMap<String, u64> {
    std::fs::read(data.join(PARENTS))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn lost(data: &Path) -> bool {
    std::fs::remove_file(data.join(LOSE)).is_ok()
}

impl Github {
    fn asked(&self, request: &str) {
        let _logged = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.data.join(REQUESTS))
            .and_then(|mut log| writeln!(log, "{request}"));
    }

    fn ids(&self) -> Ids {
        Ids::default().with(self.project, github::ISSUER)
    }

    fn record(&self, id: &IssueId) -> Result<Record, ReadFailure> {
        Ok(Record {
            id: id.clone(),
            locator: Locator {
                repository: String::from("owner/standards"),
                url: None,
                addresses: BTreeMap::new(),
            },
            created: UnixMillis(1000),
            native: kept(&self.data, &id.to_string())?,
            parent: parents(&self.data)
                .get(&id.to_string())
                .copied()
                .map(Native::Here),
            blocked_by: Some(Vec::new()),
        })
    }

    fn relating(
        &self,
        issue: &Lookup,
        change: &RelationChange,
    ) -> Result<Relating, Unrelatable> {
        let record = self.record(&issue.id).map_err(Unrelatable::Read)?;
        let plan = relate::plan(
            &self.flow,
            &self.ids(),
            issue.project,
            &record,
            change,
        )
        .map_err(|reason| Unrelatable::Unheld(reason.to_string()))?;
        let child = issue.id.to_string().parse().unwrap_or_default();
        let mut steps: Vec<RelationStep> = plan
            .changes
            .into_iter()
            .map(|edge| RelationStep {
                effect: edge.effect(),
                request: match edge {
                    relate::Change::Adopt { parent, replace } => {
                        Request::Adopt {
                            parent,
                            child,
                            replace,
                        }
                    }
                    relate::Change::Orphan(parent) => {
                        Request::Orphan { parent, child }
                    }
                    relate::Change::Block(blocker) => Request::Block(blocker),
                    relate::Change::Unblock(blocker) => {
                        Request::Unblock(blocker)
                    }
                }
                .encode(),
            })
            .collect();
        steps.extend(plan.footer);
        Ok(Relating {
            revision: record.native.revision(),
            steps,
        })
    }

    fn related(
        &self,
        issue: &Lookup,
        step: &RelationStep,
    ) -> Result<(), String> {
        let id = issue.id.to_string();
        let mut all = parents(&self.data);
        match Request::decode(&step.request).ok_or("no request")? {
            Request::Adopt { parent, .. } => {
                all.insert(id, parent);
            }
            Request::Orphan { .. } => {
                all.remove(&id);
            }
            Request::Body(body) => {
                let mut found =
                    kept(&self.data, &id).map_err(|error| error.to_string())?;
                found.body = body;
                return keep(&self.data, &id, &found);
            }
            Request::Block(_) | Request::Unblock(_) => {
                return Err(String::from("the script keeps no dependencies"));
            }
        }
        save(&self.data, PARENTS, serde_json::to_vec(&all))
    }

    fn observed(&self, id: &IssueId) -> Result<Observation, ReadFailure> {
        let mut observed = github::mapping::observe(
            &self.flow,
            &self.config,
            &self.ids(),
            self.project,
            self.record(id)?,
        );
        let posted = on(&self.data, id)
            .into_iter()
            .map(|comment| Posted {
                id: comment.id,
                author: String::from("reader"),
                created: comment.updated,
                body: comment.body,
            })
            .collect();
        github::mapping::logged(&mut observed, posted);
        Ok(observed)
    }

    fn comment(&self, issue: &Lookup, body: String) -> Result<Comment, String> {
        let mut all = comments(&self.data);
        let id = all.len() + 1;
        let comment = Comment {
            id: id.to_string(),
            url: format!(
                "https://github.com/owner/standards/issues/{}#issuecomment-{id}",
                issue.id
            ),
            body,
            updated: UnixMillis(2000),
        };
        all.push((issue.id.to_string(), comment.clone()));
        save(&self.data, COMMENTS, serde_json::to_vec(&all))?;
        Ok(comment)
    }

    fn open(&self, create: IssueCreate) -> Result<IssueId, String> {
        let next = issues(&self.data)
            .unwrap_or_default()
            .keys()
            .filter_map(|id| id.parse::<u64>().ok())
            .max()
            .unwrap_or(0)
            + 1;
        let id = next.to_string();
        keep(
            &self.data,
            &id,
            &NativeIssue {
                title: create.title,
                body: create.body,
                labels: create.labels,
                open: true,
                reason: None,
            },
        )?;
        github::ISSUER.read(&id).ok_or_else(|| String::from("id"))
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
    fn read(&self, issue: Lookup) -> IssueCompletion<'_, Observation> {
        self.asked("read");
        let observed = self.observed(&issue.id);
        Box::pin(async move { observed })
    }

    fn list(&self, _: ListPage) -> IssueCompletion<'_, Page> {
        self.asked("list");
        let page = issues(&self.data).and_then(|all| {
            let records = all
                .keys()
                .filter_map(|id| github::ISSUER.read(id))
                .map(|id| self.observed(&id))
                .collect::<Result<_, _>>()?;
            Ok(Page {
                records,
                next: None,
            })
        });
        Box::pin(async move { page })
    }

    fn comments(&self, issue: Lookup) -> IssueCompletion<'_, Vec<Comment>> {
        let found = on(&self.data, &issue.id);
        Box::pin(async move { Ok(found) })
    }

    fn write_comment(
        &self,
        issue: Lookup,
        write: CommentWrite,
    ) -> SendCompletion<'_, Comment> {
        self.asked("comment");
        let CommentWrite::Create { body } = write else {
            return Box::pin(async {
                Err(SendFailure::Rejected(ReadFailure::InvalidConfiguration))
            });
        };
        let posted = self
            .comment(&issue, body)
            .map_err(|_| SendFailure::Unknown)
            .and_then(|comment| {
                if lost(&self.data) {
                    Err(SendFailure::Unknown)
                } else {
                    Ok(comment)
                }
            });
        Box::pin(async move { posted })
    }

    fn native(&self, issue: Lookup) -> IssueCompletion<'_, NativeIssue> {
        self.asked("native");
        let native = kept(&self.data, &issue.id.to_string());
        Box::pin(async move { native })
    }

    fn writes(
        &self,
        _: &Lookup,
        observed: &NativeIssue,
        mutations: &[Mutation],
    ) -> Result<Vec<IssueWrite>, String> {
        github::mapping::writes(&self.flow, &self.config, observed, mutations)
    }

    fn write_issue(
        &self,
        issue: Lookup,
        write: IssueWrite,
    ) -> SendCompletion<'_, ()> {
        self.asked(&format!("{write:?}"));
        let id = issue.id.to_string();
        let written = kept(&self.data, &id)
            .map_err(SendFailure::Rejected)
            .and_then(|found| {
                keep(&self.data, &id, &applied(found, &write))
                    .map_err(|_| SendFailure::Unknown)
            });
        Box::pin(async move { written })
    }

    fn opening(
        &self,
        change: &Change,
        handle: Handle,
    ) -> Result<IssueCreate, String> {
        github::mapping::opening(
            &self.flow,
            &self.config,
            change,
            &handle.to_string(),
        )
    }

    fn create(
        &self,
        _: ProjectId,
        create: IssueCreate,
    ) -> SendCompletion<'_, IssueId> {
        self.asked("create");
        let opened = self
            .open(create)
            .map_err(|_| SendFailure::Unknown)
            .and_then(|id| {
                if lost(&self.data) {
                    Err(SendFailure::Unknown)
                } else {
                    Ok(id)
                }
            });
        Box::pin(async move { opened })
    }

    fn opened(
        &self,
        _: ProjectId,
        handle: Handle,
    ) -> IssueCompletion<'_, Vec<Opened>> {
        let found = issues(&self.data).map(|all| {
            all.into_iter()
                .filter(|(_, native)| {
                    github::mapping::carries(&native.body, handle)
                })
                .filter_map(|(number, native)| {
                    github::ISSUER.read(&number).map(|id| Opened { id, native })
                })
                .collect()
        });
        Box::pin(async move { found })
    }

    fn logged(&self, entry: &Entry, handle: Handle) -> String {
        github::log::comment(entry, &handle.marker())
    }

    fn plan_relation(
        &self,
        issue: Lookup,
        change: RelationChange,
    ) -> RelationCompletion<'_> {
        let planned = self.relating(&issue, &change);
        Box::pin(async move { planned })
    }

    fn write_relation(
        &self,
        issue: Lookup,
        step: RelationStep,
    ) -> SendCompletion<'_, ()> {
        self.asked(&step.effect);
        let written = self
            .related(&issue, &step)
            .map_err(|_| SendFailure::Unknown);
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
            "146",
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
        let loaded = home_files::read(&self.root, github::ISSUERS);
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
        kept(&self.root.join("data"), "146")
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
            "RemoveLabel(\"work/stage/shape\")",
            "comment"
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

impl Home {
    fn creates(&self) -> usize {
        self.requests()
            .iter()
            .filter(|request| *request == "create")
            .count()
    }

    fn opened(&self) -> Result<NativeIssue, ReadFailure> {
        kept(&self.root.join("data"), "147")
    }

    fn comments(&self, issue: &str) -> Result<Vec<String>, Box<dyn Error>> {
        Ok(on(
            &self.root.join("data"),
            &github::ISSUER.read(issue).ok_or("id")?,
        )
        .into_iter()
        .map(|comment| comment.body)
        .collect())
    }
}

#[test]
fn issue_new_on_a_github_project_sends_one_create_holding_its_marker_and_prints_the_number_github_gave()
-> Result<(), Box<dyn Error>> {
    let home = Home::new("new", &[], "Words\n")?;
    let (said, code) =
        home.home(&["issue", "new", "standards", "task", "Opened by home"])?;
    assert_eq!(said, "wrote standards/147\n");
    assert_eq!(code, 0, "{said}");
    assert_eq!(home.creates(), 1);
    let opened = home.opened()?;
    assert_eq!(opened.title, "Opened by home");
    assert_eq!(opened.labels, ["work/kind/task"]);
    let (human, footer) = opened
        .body
        .split_once("```project-data\n")
        .ok_or("no footer")?;
    assert_eq!(human, "## What\n\n## Acceptance\n\n");
    let operation = footer
        .lines()
        .find_map(|line| line.strip_prefix("operation = \""))
        .and_then(|rest| rest.strip_suffix('"'))
        .and_then(Handle::parse)
        .ok_or("no operation in the footer")?;
    assert!(github::mapping::carries(&opened.body, operation));
    let comments = home.comments("147")?;
    assert_eq!(comments.len(), 1);
    assert!(comments[0].ends_with(" note: opened.\n"), "{comments:?}");
    assert!(!home.root.join("projects/hyn162/issues").exists());
    Ok(())
}

#[test]
fn a_lost_create_reply_retried_finds_the_issue_and_creates_nothing_more()
-> Result<(), Box<dyn Error>> {
    let home = Home::new("lost-create", &[], "Words\n")?;
    std::fs::write(home.root.join("data").join(LOSE), "")?;
    let new = ["issue", "new", "standards", "task", "Opened by home"];
    let (said, code) = home.home(&new)?;
    assert!(said.contains("is unknown; retry it"), "{said}");
    assert_eq!(code, 1);
    assert_eq!(home.creates(), 1);
    let (said, code) = home.home(&new)?;
    assert_eq!(said, "wrote standards/147\n");
    assert_eq!(code, 0);
    assert_eq!(home.creates(), 0);
    assert_eq!(
        issues(&home.root.join("data"))?
            .into_keys()
            .collect::<Vec<_>>(),
        ["146", "147"]
    );
    Ok(())
}

#[test]
fn issue_note_on_a_github_issue_adds_one_comment_and_issue_show_lists_it_in_the_log()
-> Result<(), Box<dyn Error>> {
    let home = Home::new("note", &[], "Words\n")?;
    let (said, code) =
        home.home(&["issue", "note", "standards", "146", "worker", "started"])?;
    assert!(said.starts_with("wrote standards/146\n"), "{said}");
    assert_eq!(code, 0, "{said}");
    let comments = home.comments("146")?;
    assert_eq!(comments.len(), 1);
    assert!(
        comments[0].starts_with("<!-- conductor-log\nconductor-operation "),
        "{comments:?}"
    );
    let (shown, code) = home.home(&["issue", "show", "standards", "146"])?;
    assert_eq!(code, 0, "{shown}");
    let read: serde_json::Value = serde_json::from_str(&shown)?;
    let issue = &read["record"]["state"]["Valid"]["issue"];
    assert_eq!(issue["log"].as_array().map(Vec::len), Some(1), "{shown}");
    assert_eq!(issue["log"][0]["text"], "worker started");
    assert_eq!(issue["log"][0]["event"], "Note");
    assert_eq!(issue["notes"], serde_json::json!([]));
    assert_eq!(home.issue()?.body, "Words\n");
    Ok(())
}

#[test]
fn a_lost_note_reply_retried_finds_the_comment_and_posts_nothing_more()
-> Result<(), Box<dyn Error>> {
    let home = Home::new("lost-note", &[], "Words\n")?;
    std::fs::write(home.root.join("data").join(LOSE), "")?;
    let note = ["issue", "note", "standards", "146", "worker", "started"];
    let (said, code) = home.home(&note)?;
    assert!(said.contains("is unknown; retry it"), "{said}");
    assert_eq!(code, 1);
    let (said, code) = home.home(&note)?;
    assert!(said.starts_with("wrote standards/146\n"), "{said}");
    assert_eq!(code, 0, "{said}");
    let comments = home.comments("146")?;
    assert_eq!(comments.len(), 1, "{comments:?}");
    assert!(
        comments[0].ends_with(" note: worker started\n"),
        "{comments:?}"
    );
    Ok(())
}

#[test]
fn issue_set_parent_on_a_github_issue_makes_it_a_sub_issue_and_writes_no_footer()
-> Result<(), Box<dyn Error>> {
    let home = Home::new("parent", &[], "Words\n")?;
    keep(
        &home.root.join("data"),
        "140",
        &NativeIssue {
            title: String::from("The spec"),
            body: String::new(),
            labels: Vec::from([String::from("work/kind/spec")]),
            open: true,
            reason: None,
        },
    )?;
    let (said, code) =
        home.home(&["issue", "set", "standards", "146", "parent=140"])?;
    assert!(said.starts_with("wrote standards/146\n"), "{said}");
    assert_eq!(code, 0, "{said}");
    let writes: Vec<String> = home
        .requests()
        .into_iter()
        .filter(|request| {
            !["connect", "list", "read", "native"].contains(&request.as_str())
        })
        .collect();
    assert_eq!(writes, ["sub-issue of 140", "comment"]);
    assert_eq!(parents(&home.root.join("data")).get("146"), Some(&140));
    assert_eq!(home.issue()?.body, "Words\n");
    Ok(())
}

#[test]
fn issue_set_parent_to_an_issue_missing_from_a_complete_cached_listing_is_refused()
-> Result<(), Box<dyn Error>> {
    let home = Home::new("missing-parent", &[], "Words\n")?;
    let loaded = home_files::read(&home.root, github::ISSUERS);
    let project = loaded.snapshot.project("standards").ok_or("project")?.id;
    let trackers = Trackers::new(
        &home.root,
        loaded,
        Some(home.root.join("data")),
        connect,
    )?;
    let listing = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(trackers.listing(project, false))?;
    assert!(listing.status.complete, "{listing:?}");
    home.requests();
    let (said, code) =
        home.home(&["issue", "set", "standards", "146", "parent=999"])?;
    assert!(said.contains("999 is no issue"), "{said}");
    assert_eq!(code, 1);
    assert!(home.requests().iter().all(|request| {
        ["connect", "list", "read", "native"].contains(&request.as_str())
    }),);
    assert_eq!(parents(&home.root.join("data")), BTreeMap::new());
    Ok(())
}
