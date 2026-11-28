use super::{Exchange, exchanges};
use crate::{Client, Tracker, credential::Source};
use github::access::Repository;
use home::issue::{Coverage, Observation};
use home::issue::{IssueState, ListPage, Lookup, ReadFailure};
use home::{IssueId, IssueKey, ProjectId};
use project_runtime::IssueTracker as _;
use serde_json::{Value, json};

const FIRST: &str = "/repos/owner/work/issues?state=all&sort=created&direction=asc&per_page=100&page=1";
const SECOND: &str = "/repos/owner/work/issues?state=all&sort=created&direction=asc&per_page=100&page=2";
const NUMBER: &str = "/repos/owner/work/issues/7";

pub(super) fn reply(path: &'static str, status: u16, body: &Value) -> Exchange {
    Exchange {
        path,
        method: "GET",
        lost: false,
        status,
        auth: "Bearer fixture",
        headers: Vec::new(),
        body: body.to_string(),
    }
}

fn with_header(
    mut step: Exchange,
    name: &'static str,
    value: &'static str,
) -> Exchange {
    step.headers.push((name, value));
    step
}

pub(super) fn author() -> Value {
    let mut author = json!({"login":"reader","type":"User","id":1,"node_id":"user","site_admin":false,"gravatar_id":""});
    for field in [
        "avatar_url",
        "url",
        "html_url",
        "followers_url",
        "following_url",
        "gists_url",
        "starred_url",
        "subscriptions_url",
        "organizations_url",
        "repos_url",
        "events_url",
        "received_events_url",
    ] {
        author[field] = json!("https://github.com/reader");
    }
    author
}

fn issue(number: u64, id: u64) -> Value {
    let author = author();
    json!({
        "id":id,"node_id":"I_test","url":format!("{{api}}/repos/owner/work/issues/{number}"),
        "repository_url":"{api}/repos/owner/work","labels_url":"{api}/labels",
        "comments_url":"{api}/comments","events_url":"{api}/events",
        "html_url":format!("https://github.com/owner/work/issues/{number}"),
        "number":number,"state":"open","title":"Native issue","body":"Visible human text",
        "user":author,"labels":[],"assignees":[],"locked":false,"comments":3,
        "created_at":"2026-01-01T12:00:00Z","updated_at":"2026-01-02T12:00:00Z",
        "parent_issue_url":null,
        "issue_dependencies_summary":{"blocked_by":0,"blocking":0,"total_blocked_by":0,"total_blocking":0}
    })
}

fn pull() -> Value {
    let mut value = issue(7, 901);
    value["pull_request"] = json!({
        "url":"{api}/pulls/7", "html_url":"https://github.com/owner/work/pull/7",
        "diff_url":"{api}/diff","patch_url":"{api}/patch"
    });
    value
}

pub(super) fn tracker(base: &str) -> anyhow::Result<Tracker> {
    let client = Client::new(
        Repository::parse("owner/work")?,
        &Source::FineGrained {
            token: "fixture".into(),
        },
        base,
    )?;
    let flow =
        home::flow::load(include_str!("../../../home/src/flow/test-flow.toml"))
            .map_err(anyhow::Error::msg)?;
    let config =
        github::configuration::load("", &flow).map_err(anyhow::Error::msg)?;
    Ok(client.tracker(flow, config))
}

fn page(continuation: Option<String>) -> ListPage {
    ListPage {
        project: ProjectId::new(1),
        continuation,
    }
}

pub(super) fn lookup(number: &str) -> anyhow::Result<Lookup> {
    Ok(Lookup {
        project: ProjectId::new(1),
        id: IssueId::new(number).ok_or_else(|| anyhow::anyhow!("no id"))?,
    })
}

#[tokio::test]
async fn all_issue_pages_are_authenticated_and_pull_requests_are_excluded()
-> anyhow::Result<()> {
    let (base, completed) = exchanges(vec![
        with_header(
            reply(FIRST, 200, &json!([pull()])),
            "link",
            "<{api}/repositories/88/issues?page=2>; rel=\"next\"",
        ),
        reply(SECOND, 200, &json!([issue(9, 802)])),
        reply(NUMBER, 200, &pull()),
    ])
    .await?;
    let tracker = tracker(&base)?;
    let first = tracker.list(page(None)).await?;
    assert_eq!(first.records, []);
    assert!(first.next.is_some());
    let second = tracker.list(page(first.next)).await?;
    assert!(second.next.is_none());
    let record = &second.records[0];
    assert_eq!(record.id().as_str(), "9");
    assert_eq!(
        record.issue().map(|i| i.title.as_str()),
        Some("Native issue")
    );
    assert!(record.observed_at.is_some());
    assert_eq!(
        tracker.read(lookup("7")?).await,
        Err(ReadFailure::Unavailable)
    );
    completed.await??;
    Ok(())
}

#[tokio::test]
async fn redirects_verify_the_destination_and_keep_the_issue_s_number()
-> anyhow::Result<()> {
    let moved = |location| {
        with_header(reply(NUMBER, 301, &json!({})), "location", location)
    };
    let repo = || {
        reply(
            "/repos/owner/work",
            200,
            &json!({"id":88,"name":"work","full_name":"owner/work","url":"{api}/repos/owner/work"}),
        )
    };
    let (base, completed) = exchanges(vec![
        moved("{api}/repositories/88/issues/7"),
        repo(),
        reply("/repositories/88/issues/7", 200, &issue(7, 901)),
        moved("{api}/repositories/88/issues/12"),
        repo(),
        reply("/repositories/88/issues/12", 200, &issue(12, 901)),
        with_header(
            reply(NUMBER, 301, &json!({})),
            "location",
            "https://elsewhere.example/repos/owner/work/issues/12",
        ),
        with_header(
            reply(NUMBER, 301, &json!({})),
            "location",
            "{api}/repos/owner/different/issues/12",
        ),
        with_header(
            reply(NUMBER, 301, &json!({})),
            "location",
            "{api}/repositories/99/issues/12",
        ),
        repo(),
    ])
    .await?;
    let tracker = tracker(&base)?;
    let record = tracker.read(lookup("7")?).await?;
    assert_eq!(record.id().as_str(), "7");
    for _ in 0..4 {
        assert_eq!(
            tracker.read(lookup("7")?).await,
            Err(ReadFailure::IdentityConflict)
        );
    }
    completed.await??;
    Ok(())
}

#[tokio::test]
async fn permission_rate_limits_and_confirmed_deletion_remain_distinct()
-> anyhow::Result<()> {
    let error = json!({"message":"fixture response"});
    let (base, completed) = exchanges(vec![
        reply(NUMBER, 404, &error),
        reply(NUMBER, 410, &error),
        reply(FIRST, 403, &error),
        with_header(reply(FIRST, 403, &error), "x-ratelimit-remaining", "0"),
        with_header(reply(FIRST, 403, &error), "retry-after", "30"),
        reply(
            FIRST,
            403,
            &json!({"message":"You have exceeded a secondary rate limit."}),
        ),
        reply(FIRST, 429, &error),
        reply(FIRST, 401, &error),
    ])
    .await?;
    let tracker = tracker(&base)?;
    assert_eq!(
        tracker.read(lookup("7")?).await,
        Err(ReadFailure::Unavailable)
    );
    assert!(matches!(
        tracker.read(lookup("7")?).await?.state,
        IssueState::Deleted(_)
    ));
    for expected in [
        ReadFailure::PermissionDenied,
        ReadFailure::RateLimited,
        ReadFailure::RateLimited,
        ReadFailure::RateLimited,
        ReadFailure::RateLimited,
        ReadFailure::AuthenticationRefused,
    ] {
        assert_eq!(tracker.list(page(None)).await, Err(expected));
    }
    completed.await??;
    Ok(())
}

#[tokio::test]
async fn a_foreign_page_or_mismatched_issue_url_cannot_replace_the_project_snapshot()
-> anyhow::Result<()> {
    let list = json!([issue(7, 901)]);
    let foreign = |link| with_header(reply(FIRST, 200, &list), "link", link);
    let mut wrong_repository = issue(7, 901);
    wrong_repository["repository_url"] = json!("{api}/repos/owner/transferred");
    let (base, completed) = exchanges(vec![
        foreign("<https://elsewhere.example/repos/owner/work/issues?page=2>; rel=\"next\""),
        foreign("<{api}/repos/elsewhere/work/issues?page=2>; rel=\"next\""),
        foreign("<{api}/repos/owner/work/issues?page=1>; rel=\"next\""),
        reply(NUMBER, 200, &wrong_repository),
        reply(NUMBER, 200, &issue(3,901)),
    ]).await?;
    let tracker = tracker(&base)?;
    for _ in 0..3 {
        assert_eq!(
            tracker.list(page(None)).await,
            Err(ReadFailure::InvalidResponse)
        );
    }
    for _ in 0..2 {
        assert_eq!(
            tracker.read(lookup("7")?).await,
            Err(ReadFailure::IdentityConflict)
        );
    }
    completed.await??;
    Ok(())
}

#[tokio::test]
async fn an_unrecognized_native_reason_preserves_the_issue_and_its_diagnostic()
-> anyhow::Result<()> {
    let mut unusual = issue(7, 901);
    unusual["state"] = json!("closed");
    unusual["state_reason"] = json!("future_reason");
    let (base, completed) =
        exchanges(vec![reply(NUMBER, 200, &unusual)]).await?;
    let observed = tracker(&base)?.read(lookup("7")?).await?;
    assert_eq!(observed.id().as_str(), "7");
    assert!(
        matches!(observed.state, IssueState::Invalid(_))
            || !observed.findings.is_empty()
    );
    completed.await??;
    Ok(())
}

#[tokio::test]
async fn only_the_issue_s_number_can_address_an_issue() -> anyhow::Result<()> {
    let (base, completed) = exchanges(Vec::new()).await?;
    completed.await??;
    let tracker = tracker(&base)?;
    for alias in ["#7", "07", "+7", "0", "-7", " 7"] {
        assert_eq!(
            tracker.read(lookup(alias)?).await,
            Err(ReadFailure::InvalidResponse),
            "{alias}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn an_issue_s_number_is_its_id_and_its_api_id_is_among_its_addresses()
-> anyhow::Result<()> {
    let (base, completed) = exchanges(vec![reply(
        "/repos/owner/work/issues/146",
        200,
        &issue(146, 6_663_214_162),
    )])
    .await?;
    let record = tracker(&base)?.read(lookup("146")?).await?;
    assert_eq!(record.id().as_str(), "146");
    assert_eq!(
        record
            .locator
            .as_ref()
            .and_then(|locator| locator.addresses.get("id"))
            .map(String::as_str),
        Some("6663214162")
    );
    completed.await??;
    Ok(())
}

const BLOCKERS: &str =
    "/repos/owner/work/issues/7/dependencies/blocked_by?per_page=100";

fn sub_issue_of(parent: &str) -> Value {
    let mut value = issue(7, 901);
    value["parent_issue_url"] = json!(parent);
    value
}

fn blocked(total: u32) -> Value {
    let mut value = issue(7, 901);
    value["issue_dependencies_summary"]["total_blocked_by"] = json!(total);
    value
}

fn targets(observed: &Observation, relation: &str) -> Vec<String> {
    observed
        .issue()
        .and_then(|issue| issue.links.get(relation))
        .map_or_else(Vec::new, |keys| {
            keys.iter().map(ToString::to_string).collect()
        })
}

#[tokio::test]
async fn a_sub_issue_reads_its_parent_issue_as_its_parent() -> anyhow::Result<()>
{
    let (base, completed) = exchanges(vec![reply(
        NUMBER,
        200,
        &sub_issue_of("{api}/repos/owner/work/issues/3"),
    )])
    .await?;
    let observed = tracker(&base)?.read(lookup("7")?).await?;
    let parent = IssueKey::named("3", ProjectId::new(1))
        .ok_or_else(|| anyhow::anyhow!("no key"))?;
    assert_eq!(targets(&observed, "parent"), [parent.to_string()]);
    assert_eq!(
        observed.completeness.relations.get("parent"),
        Some(&Coverage::Complete)
    );
    completed.await??;
    Ok(())
}

#[tokio::test]
async fn an_issue_with_blocked_by_dependencies_reads_them_in_blocked_by()
-> anyhow::Result<()> {
    let (base, completed) = exchanges(vec![
        reply(FIRST, 200, &json!([blocked(2)])),
        reply(BLOCKERS, 200, &json!([issue(5, 905), issue(6, 906)])),
    ])
    .await?;
    let listed = tracker(&base)?.list(page(None)).await?;
    let observed = &listed.records[0];
    let project = ProjectId::new(1);
    assert_eq!(
        targets(observed, "blocked_by"),
        [format!("{project}/5"), format!("{project}/6")]
    );
    assert_eq!(
        observed.completeness.relations.get("blocked_by"),
        Some(&Coverage::Complete)
    );
    completed.await??;
    Ok(())
}

#[tokio::test]
async fn a_footer_relation_reads_beside_the_native_relations()
-> anyhow::Result<()> {
    let mut value = sub_issue_of("{api}/repos/owner/work/issues/3");
    value["issue_dependencies_summary"]["total_blocked_by"] = json!(1);
    value["body"] = json!(
        "Visible human text\n\n```project-data\nschema = 1\n\n[relations]\nblocked_by = [\"local~0v2csjdj\"]\n```"
    );
    let (base, completed) = exchanges(vec![
        reply(NUMBER, 200, &value),
        reply(BLOCKERS, 200, &json!([issue(5, 905)])),
    ])
    .await?;
    let observed = tracker(&base)?.read(lookup("7")?).await?;
    let project = ProjectId::new(1);
    assert_eq!(targets(&observed, "parent"), [format!("{project}/3")]);
    assert_eq!(
        targets(&observed, "blocked_by"),
        [format!("{project}/5"), format!("{project}/local~0v2csjdj")]
    );
    completed.await??;
    Ok(())
}

#[tokio::test]
async fn a_native_relation_in_another_repository_is_reported_and_left_uncovered()
-> anyhow::Result<()> {
    let (base, completed) = exchanges(vec![reply(
        NUMBER,
        200,
        &sub_issue_of("{api}/repos/owner/other/issues/3"),
    )])
    .await?;
    let observed = tracker(&base)?.read(lookup("7")?).await?;
    assert!(observed.issue().is_some());
    assert_eq!(targets(&observed, "parent"), Vec::<String>::new());
    assert!(matches!(
        observed.completeness.relations.get("parent"),
        Some(Coverage::Incomplete { .. })
    ));
    assert!(
        observed
            .findings
            .iter()
            .any(|finding| finding.message.contains("owner/other#3"))
    );
    completed.await??;
    Ok(())
}
