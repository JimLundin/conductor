use std::fmt::Write as _;
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};

use home::backend::{Backends, CredentialRef, CredentialState, Selection};
use home::canvas::{
    Block, Canvas, CanvasEntry, CanvasFile, CanvasKey, CanvasName, Stray,
    StrayReason,
};
use home::flow::Flow;
use home::issue::ReadMode;
use home::{IssueId, ProjectId, UnixMillis};
use home_store::{CanvasStore, Plant, Putting, StoreError};
use http_body_util::{BodyExt as _, Full};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response};
use hyper_util::rt::TokioIo;
use project_runtime::issues::IssueCache;
use project_runtime::operations::Operations;
use project_runtime::tracker::TrackerStore as _;
use project_runtime::{Registration, Registry};
use serde_json::{Value, json};

use super::tracking::author;
use crate::{Canvases, HostCredentials};

const ALPHA: ProjectId = ProjectId::new(1);
const BETA: ProjectId = ProjectId::new(2);

struct Posted {
    repo: &'static str,
    issue: u64,
    id: u64,
    body: String,
    updated: u64,
}

#[derive(Default)]
struct State {
    comments: Vec<Posted>,
    clock: u64,
    requests: Vec<(Method, String)>,
    lost: u32,
    lagging: u32,
}

impl State {
    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }
}

struct Github {
    base: String,
    asks: mpsc::UnboundedSender<Ask>,
    tasks: [tokio::task::JoinHandle<()>; 2],
}

impl Drop for Github {
    fn drop(&mut self) {
        self.tasks.iter().for_each(tokio::task::JoinHandle::abort);
    }
}

fn time(millis: u64) -> String {
    format!(
        "1970-01-01T00:{:02}:{:02}.{:03}Z",
        millis / 60_000,
        millis / 1000 % 60,
        millis % 1000
    )
}

fn comment(base: &str, posted: &Posted) -> Value {
    let (repo, issue, id) = (posted.repo, posted.issue, posted.id);
    json!({
        "id": id, "node_id": format!("IC_{id}"),
        "url": format!("{base}/repos/owner/{repo}/issues/comments/{id}"),
        "html_url": format!("https://github.com/owner/{repo}/issues/{issue}#issuecomment-{id}"),
        "issue_url": format!("{base}/repos/owner/{repo}/issues/{issue}"),
        "body": posted.body, "user": author(),
        "created_at": time(0), "updated_at": time(posted.updated)
    })
}

fn issue(base: &str, repo: &str, number: u64) -> Value {
    json!({
        "id": number + 900, "node_id": "I_test",
        "url": format!("{base}/repos/owner/{repo}/issues/{number}"),
        "repository_url": format!("{base}/repos/owner/{repo}"),
        "labels_url": "https://github.com/labels", "comments_url": "https://github.com/comments",
        "events_url": "https://github.com/events",
        "html_url": format!("https://github.com/owner/{repo}/issues/{number}"),
        "number": number, "state": "open", "title": "Native issue", "body": "Visible human text",
        "user": author(), "labels": [], "assignees": [], "locked": false, "comments": 0,
        "created_at": "2026-01-01T12:00:00Z", "updated_at": "2026-01-02T12:00:00Z"
    })
}

fn answer(
    state: &mut State,
    base: &str,
    method: &Method,
    path: &str,
    sent: &Value,
) -> (u16, Value) {
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match (method, parts.as_slice()) {
        (&Method::GET, ["repos", "owner", repo, "issues"]) => (
            200,
            Value::Array(
                [1, 2].iter().map(|n| issue(base, repo, *n)).collect(),
            ),
        ),
        (&Method::GET, ["repos", "owner", repo, "issues", "comments"]) => (
            200,
            Value::Array(
                state
                    .comments
                    .iter()
                    .filter(|posted| posted.repo == *repo)
                    .map(|posted| comment(base, posted))
                    .collect(),
            ),
        ),
        (
            &Method::GET,
            ["repos", "owner", repo, "issues", number, "comments"],
        ) => (200, {
            let lags = state.lagging > 0;
            state.lagging = state.lagging.saturating_sub(1);
            let shown = state.comments.len() - usize::from(lags);
            Value::Array(
                state.comments[..shown]
                    .iter()
                    .filter(|posted| {
                        posted.repo == *repo
                            && posted.issue.to_string() == *number
                    })
                    .map(|posted| comment(base, posted))
                    .collect(),
            )
        }),
        (
            &Method::POST,
            ["repos", "owner", repo, "issues", number, "comments"],
        ) => {
            let updated = state.tick();
            let posted = Posted {
                repo: if *repo == "alpha" { "alpha" } else { "beta" },
                issue: number.parse().unwrap_or_default(),
                id: 100 + state.comments.len() as u64,
                body: sent["body"].as_str().unwrap_or_default().to_owned(),
                updated,
            };
            let made = comment(base, &posted);
            state.comments.push(posted);
            (201, made)
        }
        (&Method::PATCH, ["repos", "owner", _, "issues", "comments", id]) => {
            let updated = state.tick();
            let Some(posted) = state
                .comments
                .iter_mut()
                .find(|posted| posted.id.to_string() == *id)
            else {
                return (404, json!({"message": "Not Found"}));
            };
            sent["body"]
                .as_str()
                .unwrap_or_default()
                .clone_into(&mut posted.body);
            posted.updated = updated;
            let edited = comment(base, posted);
            (200, edited)
        }
        _ => (404, json!({"message": "Not Found"})),
    }
}

type Ask = Box<dyn FnOnce(&mut State) + Send>;

async fn asked<T: Send + 'static>(
    asks: &mpsc::UnboundedSender<Ask>,
    change: impl FnOnce(&mut State) -> T + Send + 'static,
) -> anyhow::Result<T> {
    let (reply, answered) = oneshot::channel();
    asks.send(Box::new(move |state: &mut State| {
        let _sent = reply.send(change(state));
    }))
    .map_err(|_| anyhow::anyhow!("the fake GitHub stopped"))?;
    Ok(answered.await?)
}

async fn served(
    asks: mpsc::UnboundedSender<Ask>,
    base: String,
    request: Request<Incoming>,
) -> anyhow::Result<Response<Full<Bytes>>> {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let body = request.into_body().collect().await?.to_bytes();
    let sent = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
    let (status, reply, lost) = asked(&asks, move |state| {
        state.requests.push((method.clone(), path.clone()));
        let (status, reply) = answer(state, &base, &method, &path, &sent);
        let lost = method == Method::POST && state.lost > 0;
        state.lost -= u32::from(lost);
        (status, reply, lost)
    })
    .await?;
    anyhow::ensure!(!lost, "the reply is lost");
    let mut response = Response::new(Full::new(Bytes::from(reply.to_string())));
    *response.status_mut() = hyper::StatusCode::from_u16(status)?;
    Ok(response)
}

impl Github {
    async fn start() -> anyhow::Result<Self> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let base = format!("http://{}", listener.local_addr()?);
        let (asks, mut inbox) = mpsc::unbounded_channel::<Ask>();
        let keeper = tokio::spawn(async move {
            let mut state = State::default();
            while let Some(ask) = inbox.recv().await {
                ask(&mut state);
            }
        });
        let shared = asks.clone();
        let at = base.clone();
        let listening = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let (connection_asks, connection_base) =
                    (shared.clone(), at.clone());
                let serve = service_fn(move |request: Request<Incoming>| {
                    served(
                        connection_asks.clone(),
                        connection_base.clone(),
                        request,
                    )
                });
                tokio::spawn(
                    http1::Builder::new()
                        .serve_connection(TokioIo::new(socket), serve),
                );
            }
        });
        Ok(Self {
            base,
            asks,
            tasks: [keeper, listening],
        })
    }

    async fn with<T: Send + 'static>(
        &self,
        change: impl FnOnce(&mut State) -> T + Send + 'static,
    ) -> anyhow::Result<T> {
        asked(&self.asks, change).await
    }

    async fn post(
        &self,
        repo: &'static str,
        issue: u64,
        body: String,
    ) -> anyhow::Result<String> {
        self.with(move |state| {
            let updated = state.tick();
            let id = 100 + state.comments.len() as u64;
            state.comments.push(Posted {
                repo,
                issue,
                id,
                body,
                updated,
            });
            format!("https://github.com/owner/{repo}/issues/{issue}#issuecomment-{id}")
        })
        .await
    }

    async fn seed(
        &self,
        repo: &'static str,
        issue: u64,
        body: String,
    ) -> anyhow::Result<String> {
        self.with(move |state| {
            let id = 100 + state.comments.len() as u64;
            state.comments.push(Posted {
                repo,
                issue,
                id,
                body,
                updated: state.clock,
            });
            format!("https://github.com/owner/{repo}/issues/{issue}#issuecomment-{id}")
        })
        .await
    }

    async fn requests(&self) -> anyhow::Result<Vec<(Method, String)>> {
        self.with(|state| state.requests.clone()).await
    }

    async fn bodies(&self) -> anyhow::Result<Vec<String>> {
        self.with(|state| {
            state
                .comments
                .iter()
                .map(|posted| posted.body.clone())
                .collect()
        })
        .await
    }
}

fn flow() -> anyhow::Result<Flow> {
    home::flow::load(include_str!("../../../home/src/flow/test-flow.toml"))
        .map_err(anyhow::Error::msg)
}

fn backends(repo: &str) -> Backends {
    Backends {
        tracker: Some(Selection {
            adapter: String::from("github"),
            resource: format!("owner/{repo}"),
            credential: CredentialRef::try_from(format!("host/{repo}")).ok(),
            options: String::new(),
        }),
        ..Backends::default()
    }
}

struct Host {
    dir: tempfile::TempDir,
}

impl Host {
    fn new() -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("project-credentials.toml");
        let mut entries = String::new();
        for repo in ["alpha", "beta"] {
            write!(
                entries,
                "[\"host/{repo}\"]\nrepository = \"owner/{repo}\"\nwrite_issues = true\n[\"host/{repo}\".source]\nkind = \"fine_grained\"\ntoken = \"fixture\"\n"
            )?;
        }
        std::fs::write(&path, entries)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(
                &path,
                std::fs::Permissions::from_mode(0o600),
            )?;
        }
        Operations::bind(dir.path(), [7; 32])?;
        Ok(Self { dir })
    }

    fn store(
        &self,
        project: ProjectId,
        repo: &str,
        github: &Github,
    ) -> anyhow::Result<Canvases> {
        Canvases::at(
            self.dir.path(),
            project,
            backends(repo),
            flow()?,
            &github.base,
        )
        .map_err(anyhow::Error::msg)
    }

    async fn refresh(
        &self,
        project: ProjectId,
        repo: &str,
        github: &Github,
    ) -> anyhow::Result<()> {
        let backends = backends(repo);
        let selection = backends
            .tracker
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no tracker"))?;
        let flow = flow()?;
        let reader = HostCredentials::new(self.dir.path())
            .connect_at(&selection, &github.base)?
            .tracker(
                flow.clone(),
                github::configuration::load("", &flow)
                    .map_err(anyhow::Error::msg)?,
            );
        let mut registry = Registry::default();
        registry
            .register_tracker(
                Registration {
                    selection,
                    capabilities: reader.capabilities(),
                    credential: CredentialState::Available,
                },
                Arc::new(reader),
            )
            .map_err(|refusal| anyhow::anyhow!("{refusal:?}"))?;
        let listing = registry
            .project(&backends)
            .issues(
                &IssueCache::new(self.dir.path(), &flow),
                project,
                ReadMode::Refresh,
            )
            .await?;
        anyhow::ensure!(
            listing.status.failure.is_none(),
            "{:?}",
            listing.status
        );
        self.store(project, repo, github)?.refresh(&listing).await?;
        Ok(())
    }
}

fn named(name: &str) -> anyhow::Result<CanvasName> {
    CanvasName::parse(name).map_err(|error| anyhow::anyhow!("{error}"))
}

fn key(
    project: ProjectId,
    issue: &str,
    name: &str,
) -> anyhow::Result<CanvasKey> {
    Ok(CanvasKey {
        project,
        issue: IssueId::new(issue).ok_or_else(|| anyhow::anyhow!("issue"))?,
        name: named(name)?,
    })
}

fn drawn(text: &str) -> anyhow::Result<Canvas> {
    Canvas::new(
        String::from("Kept on GitHub"),
        vec![Block::Prose {
            text: String::from(text),
        }],
    )
    .map_err(|error| anyhow::anyhow!("{error}"))
}

fn file(written: u64, text: &str) -> anyhow::Result<CanvasFile> {
    Ok(CanvasFile {
        written: UnixMillis(written),
        by: String::from("devbox·claude"),
        canvas: drawn(text)?,
    })
}

fn body(name: &str, text: &str) -> anyhow::Result<String> {
    github::canvas::comment(
        &named(name)?,
        "devbox·claude",
        "conductor-operation 07/1",
        &drawn(text)?,
    )
    .map_err(|error| anyhow::anyhow!("{error}"))
}

fn writes(requests: &[(Method, String)]) -> Vec<(Method, String)> {
    requests
        .iter()
        .filter(|(method, _)| *method != Method::GET)
        .cloned()
        .collect()
}

async fn spoil(github: &Github, key: &CanvasKey) -> anyhow::Result<String> {
    let good = body(key.name.as_str(), "spoiled")?;
    let (head, _) = good
        .split_once("<details>")
        .ok_or_else(|| anyhow::anyhow!("no data"))?;
    github
        .seed("alpha", key.issue.as_str().parse()?, head.to_owned())
        .await
}

async fn unnamed(github: &Github, issue: &IssueId) -> anyhow::Result<String> {
    let good = body("design", "unnamed")?;
    github
        .seed(
            "alpha",
            issue.as_str().parse()?,
            good.replace("name: design", "name: Bad Name"),
        )
        .await
}

struct Repositories {
    alpha: Canvases,
    beta: Canvases,
}

impl Repositories {
    fn of(&self, project: ProjectId) -> &Canvases {
        if project == ALPHA {
            &self.alpha
        } else {
            &self.beta
        }
    }
}

impl CanvasStore for Repositories {
    fn all(&self) -> Result<home::canvas::Held<CanvasKey>, StoreError> {
        let mut held = self.alpha.all()?;
        let beta = self.beta.all()?;
        held.canvases.extend(beta.canvases);
        held.strays.extend(beta.strays);
        held.canvases.sort();
        Ok(held)
    }

    fn list(
        &self,
        project: &ProjectId,
        issue: &IssueId,
    ) -> Result<
        home::canvas::Held<Result<CanvasEntry, home::canvas::Broken>>,
        StoreError,
    > {
        self.of(*project).list(project, issue)
    }

    fn get(&self, key: &CanvasKey) -> Result<Option<CanvasFile>, StoreError> {
        self.of(key.project).get(key)
    }

    fn put<'a>(
        &'a mut self,
        key: &'a CanvasKey,
        file: &'a CanvasFile,
    ) -> Putting<'a> {
        if key.project == ALPHA {
            self.alpha.put(key, file)
        } else {
            self.beta.put(key, file)
        }
    }
}

#[tokio::test]
async fn github_s_store_keeps_the_store_contract() -> anyhow::Result<()> {
    let github = Github::start().await?;
    let host = Host::new()?;
    let mut stores = Repositories {
        alpha: host.store(ALPHA, "alpha", &github)?,
        beta: host.store(BETA, "beta", &github)?,
    };
    let kept = home_store::contract(
        &mut stores,
        async |_, key| {
            let spoiled = spoil(&github, key).await;
            assert!(spoiled.is_ok(), "{spoiled:?}");
            let refreshed = host.refresh(ALPHA, "alpha", &github).await;
            assert!(refreshed.is_ok(), "{refreshed:?}");
        },
        async |_, plant| match plant {
            Plant::Unnamed { issue, .. } => {
                let place = unnamed(&github, issue).await;
                let refreshed = host.refresh(ALPHA, "alpha", &github).await;
                assert!(refreshed.is_ok(), "{refreshed:?}");
                place.ok()
            }
            Plant::Folder { .. } | Plant::NotAnIssue(_) => None,
        },
    )
    .await;
    assert_eq!(kept, Ok(()));
    Ok(())
}

#[tokio::test]
async fn a_put_of_a_new_canvas_creates_one_comment_and_a_second_put_edits_it()
-> anyhow::Result<()> {
    let github = Github::start().await?;
    let host = Host::new()?;
    let mut store = host.store(ALPHA, "alpha", &github)?;
    let design = key(ALPHA, "1", "design")?;
    store.put(&design, &file(1, "The first draft.")?).await?;
    let created = github.bodies().await?;
    store.put(&design, &file(2, "The second draft.")?).await?;
    assert_eq!(
        writes(&github.requests().await?),
        [
            (
                Method::POST,
                String::from("/repos/owner/alpha/issues/1/comments")
            ),
            (
                Method::PATCH,
                String::from("/repos/owner/alpha/issues/comments/100")
            ),
        ]
    );
    let [first] = created.as_slice() else {
        anyhow::bail!("created {created:?}");
    };
    assert!(first.starts_with(
        "<!-- conductor-canvas\nname: design\nby: \"devbox·claude\"\nconductor-operation 0707070707070707070707070707070707070707070707070707070707070707/"
    ));
    assert!(first.contains("\n# Kept on GitHub\n\nThe first draft.\n"));
    assert!(first.contains(&format!(
        "```json\n{}\n```",
        drawn("The first draft.")?.encode()
    )));
    let edited = github.bodies().await?;
    assert_eq!(edited.len(), 1);
    assert_eq!(
        store.get(&design)?.map(|kept| kept.canvas),
        Some(drawn("The second draft.")?)
    );
    Ok(())
}

#[tokio::test]
async fn a_marked_comment_whose_name_doesnt_parse_is_a_stray_placed_by_its_url()
-> anyhow::Result<()> {
    let github = Github::start().await?;
    let host = Host::new()?;
    let good = body("design", "Fine.")?;
    let unnamed = github
        .post(
            "alpha",
            1,
            good.replace("name: design", "name: Design Tree"),
        )
        .await?;
    github
        .post("alpha", 1, String::from("A person's note"))
        .await?;
    host.refresh(ALPHA, "alpha", &github).await?;
    let store = host.store(ALPHA, "alpha", &github)?;
    let one = IssueId::new("1").ok_or_else(|| anyhow::anyhow!("issue"))?;
    let listed = store.list(&ALPHA, &one)?;
    assert!(listed.canvases.is_empty(), "{:?}", listed.canvases);
    assert_eq!(
        listed.strays,
        [Stray {
            place: unnamed,
            reason: StrayReason::Unnamed
        }]
    );
    Ok(())
}

#[tokio::test]
async fn a_put_of_a_broken_canvas_edits_its_comment_and_creates_none()
-> anyhow::Result<()> {
    let github = Github::start().await?;
    let host = Host::new()?;
    let good = body("design", "Fine.")?;
    let (head, _) = good
        .split_once("<details>")
        .ok_or_else(|| anyhow::anyhow!("no data"))?;
    github.post("alpha", 1, head.to_owned()).await?;
    host.refresh(ALPHA, "alpha", &github).await?;
    let mut store = host.store(ALPHA, "alpha", &github)?;
    let design = key(ALPHA, "1", "design")?;
    store.put(&design, &file(2, "Mended.")?).await?;
    assert_eq!(
        writes(&github.requests().await?),
        [(
            Method::PATCH,
            String::from("/repos/owner/alpha/issues/comments/100")
        )]
    );
    assert_eq!(github.bodies().await?.len(), 1);
    assert_eq!(
        store.get(&design)?.map(|kept| kept.canvas),
        Some(drawn("Mended.")?)
    );
    assert_eq!(store.all()?.strays, []);
    Ok(())
}

#[tokio::test]
async fn a_canvas_over_github_s_comment_limit_fails_with_its_size_and_sends_no_request()
-> anyhow::Result<()> {
    let github = Github::start().await?;
    let host = Host::new()?;
    let mut store = host.store(ALPHA, "alpha", &github)?;
    let design = key(ALPHA, "1", "design")?;
    let long = file(1, &"word ".repeat(github::canvas::LIMIT / 4))?;
    let refused = store.put(&design, &long).await;
    let Err(StoreError::Refused { key: at, reason }) = refused else {
        anyhow::bail!("{refused:?}");
    };
    assert_eq!(at, design);
    let size: usize = reason
        .strip_prefix("its comment would be ")
        .and_then(|rest| rest.split_once(' '))
        .and_then(|(size, _)| size.parse().ok())
        .ok_or_else(|| anyhow::anyhow!("{reason}"))?;
    assert!(size > github::canvas::LIMIT, "{reason}");
    assert!(reason.ends_with("over GitHub's limit of 65536: split the canvas"));
    assert_eq!(github.requests().await?, []);
    Ok(())
}

#[tokio::test]
async fn after_a_refresh_get_and_list_answer_offline_with_the_comment_s_time_and_signer()
-> anyhow::Result<()> {
    let github = Github::start().await?;
    let host = Host::new()?;
    github.with(|state| state.clock = 41_999).await?;
    github
        .post("alpha", 2, body("design", "Read offline.")?)
        .await?;
    host.refresh(ALPHA, "alpha", &github).await?;
    let offline = host.store(ALPHA, "alpha", &github)?;
    drop(github);
    let design = key(ALPHA, "2", "design")?;
    assert_eq!(
        offline.get(&design)?,
        Some(CanvasFile {
            written: UnixMillis(42_000),
            by: String::from("devbox·claude"),
            canvas: drawn("Read offline.")?,
        })
    );
    assert_eq!(
        offline.list(&ALPHA, &design.issue)?.canvases,
        [Ok(CanvasEntry {
            name: design.name.clone(),
            written: UnixMillis(42_000),
            by: String::from("devbox·claude"),
        })]
    );
    Ok(())
}

#[tokio::test]
async fn an_edit_on_github_to_the_markdown_is_replaced_on_the_next_put_and_leaves_get_as_it_was()
-> anyhow::Result<()> {
    let github = Github::start().await?;
    let host = Host::new()?;
    let mut store = host.store(ALPHA, "alpha", &github)?;
    let design = key(ALPHA, "1", "design")?;
    store.put(&design, &file(1, "As put.")?).await?;
    github
        .with(|state| {
            for posted in &mut state.comments {
                posted.body = posted
                    .body
                    .replace("\nAs put.\n", "\nEdited by hand on GitHub.\n")
                    .replace('\n', "\r\n");
            }
        })
        .await?;
    host.refresh(ALPHA, "alpha", &github).await?;
    assert_eq!(
        store.get(&design)?.map(|kept| kept.canvas),
        Some(drawn("As put.")?)
    );
    store.put(&design, &file(2, "As put.")?).await?;
    let bodies = github.bodies().await?;
    assert_eq!(bodies.len(), 1);
    assert!(
        bodies.iter().all(|body| !body.contains("Edited by hand")
            && body.contains("\n# Kept on GitHub\n\nAs put.\n")),
        "{bodies:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_second_comment_holding_the_same_canvas_is_a_stray_and_the_first_is_the_canvas()
-> anyhow::Result<()> {
    let github = Github::start().await?;
    let host = Host::new()?;
    github
        .post("alpha", 1, body("design", "The first.")?)
        .await?;
    let again = github.post("alpha", 1, body("design", "A copy.")?).await?;
    host.refresh(ALPHA, "alpha", &github).await?;
    let store = host.store(ALPHA, "alpha", &github)?;
    let design = key(ALPHA, "1", "design")?;
    assert_eq!(
        store.get(&design)?.map(|kept| kept.canvas),
        Some(drawn("The first.")?)
    );
    assert_eq!(
        store.all()?.strays,
        [Stray {
            place: again,
            reason: StrayReason::Repeated
        }]
    );
    Ok(())
}

#[tokio::test]
async fn a_put_whose_create_reply_is_lost_put_again_finds_its_marker_and_leaves_one_comment()
-> anyhow::Result<()> {
    let github = Github::start().await?;
    let host = Host::new()?;
    let mut store = host.store(ALPHA, "alpha", &github)?;
    let design = key(ALPHA, "1", "design")?;
    github.with(|state| state.lost = 1).await?;
    let lost = store.put(&design, &file(1, "The first draft.")?).await;
    assert!(
        matches!(&lost, Err(StoreError::Unavailable { reason }) if reason.contains("is unknown")),
        "{lost:?}"
    );
    store.put(&design, &file(2, "The second draft.")?).await?;
    assert_eq!(
        writes(&github.requests().await?),
        [
            (
                Method::POST,
                String::from("/repos/owner/alpha/issues/1/comments")
            ),
            (
                Method::PATCH,
                String::from("/repos/owner/alpha/issues/comments/100")
            ),
        ]
    );
    assert_eq!(github.bodies().await?.len(), 1);
    assert_eq!(
        store.get(&design)?.map(|kept| kept.canvas),
        Some(drawn("The second draft.")?)
    );
    Ok(())
}

#[tokio::test]
async fn an_uncertain_create_unseen_while_github_s_list_lags_sends_no_second_create()
-> anyhow::Result<()> {
    let github = Github::start().await?;
    let host = Host::new()?;
    let mut store = host.store(ALPHA, "alpha", &github)?;
    let design = key(ALPHA, "1", "design")?;
    github.with(|state| state.lost = 1).await?;
    let lost = store.put(&design, &file(1, "The first draft.")?).await;
    assert!(lost.is_err(), "{lost:?}");
    github.with(|state| state.lagging = 2).await?;
    let lagged = store.put(&design, &file(2, "The second draft.")?).await;
    assert!(
        matches!(&lagged, Err(StoreError::Unavailable { reason }) if reason.contains("is unknown")),
        "{lagged:?}"
    );
    github.with(|state| state.lagging = 0).await?;
    store.put(&design, &file(3, "The third draft.")?).await?;
    assert_eq!(
        writes(&github.requests().await?),
        [
            (
                Method::POST,
                String::from("/repos/owner/alpha/issues/1/comments")
            ),
            (
                Method::PATCH,
                String::from("/repos/owner/alpha/issues/comments/100")
            ),
        ]
    );
    assert_eq!(github.bodies().await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_refresh_keeps_the_canvases_of_listed_issues_only()
-> anyhow::Result<()> {
    let github = Github::start().await?;
    let host = Host::new()?;
    github.post("alpha", 2, body("design", "Listed.")?).await?;
    github
        .post("alpha", 7, body("design", "On a pull request.")?)
        .await?;
    host.refresh(ALPHA, "alpha", &github).await?;
    let store = host.store(ALPHA, "alpha", &github)?;
    assert_eq!(store.all()?.canvases, [key(ALPHA, "2", "design")?]);
    Ok(())
}
