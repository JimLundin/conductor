use crate::{ProjectId, UnixMillis};
use alloc::collections::BTreeMap;

use super::Snapshot;
use crate::issue::storage::File;
use crate::issue::storage::{Inputs, decode};

const FLOW: &str = include_str!("../test-flow.toml");
const FOLDER: &str = "7w85je";
const TASK: &str = "0tm7zx0d";

fn folder() -> ProjectId {
    ProjectId::parse(FOLDER).unwrap_or(ProjectId::new(0))
}

fn task(status: &str) -> File {
    File {
        project: ProjectId::parse(FOLDER).unwrap_or(ProjectId::new(0)),
        stem: String::from(TASK),
        text: format!(
            "+++\nid = \"{TASK}\"\ntitle = \"Write it\"\nkind = \"task\"\n\
             status = \"{status}\"\n+++\n"
        ),
    }
}

fn inputs(flow: Result<&str, &str>) -> Inputs {
    Inputs {
        unavailable: Vec::new(),
        observed_at: None,
        flow: flow.map(String::from).map_err(String::from),
        projects: Vec::from([
            (
                String::from(FOLDER),
                Ok(String::from(
                    "name = \"home\"\ntitle = \"Home\"\n\
                     root = \"e22b7f7b2976650400965d4147ba771675f96076\"\n",
                )),
            ),
            (String::from("Bad"), Ok(String::new())),
        ]),
        issues: Vec::from([task("open")]),
        unread: Vec::from([String::from("projects/x/issues/y.md: denied")]),
    }
}

fn statuses(home: &Snapshot) -> Vec<&str> {
    home.graph()
        .issues
        .iter()
        .map(|issue| issue.status.as_str())
        .collect()
}

#[test]
fn a_home_reads_its_projects_and_issues_and_says_what_it_could_not_read() {
    let home = crate::issue::storage::snapshot(inputs(Ok(FLOW)));
    let named = |name: &str| {
        home.project(name)
            .map(|project| (project.id.to_string(), project.name.as_str()))
    };
    assert_eq!(named("home"), Some((String::from(FOLDER), "home")));
    assert_eq!(named(FOLDER), Some((String::from(FOLDER), "home")));
    assert_eq!(named("Bad"), None);
    assert_eq!(statuses(&home), ["open"]);
    assert_eq!(
        home.problems(),
        [
            "projects/Bad: the folder's name is not a project id of 6 base 32 digits",
            "projects/x/issues/y.md: denied",
        ]
    );
}

#[test]
fn a_flow_that_does_not_load_reads_no_issues_and_says_where_it_is() {
    let home = crate::issue::storage::snapshot(inputs(Err("not found")));
    assert_eq!(home.flow(), Err("docs/agents/flow.toml: not found"));
    assert_eq!(statuses(&home), Vec::<&str>::new());
    assert_eq!(
        home.problems().first().map(String::as_str),
        Some("docs/agents/flow.toml: not found")
    );
}

#[test]
fn a_home_with_a_changed_issue_replaces_its_previous_observation()
-> Result<(), String> {
    let home = crate::issue::storage::snapshot(inputs(Ok(FLOW)));
    let changed = decode(home.flow()?, &task("ready"))
        .map_err(|problem| problem.to_string())?;
    let moved = home.with(changed.0);
    assert_eq!(moved.records().len(), 1);
    assert_eq!(statuses(&moved), ["ready"]);
    assert_eq!(statuses(&home), ["open"]);
    Ok(())
}

#[test]
fn an_invalid_record_remains_addressable() {
    let mut input = inputs(Ok(FLOW));
    input.issues = Vec::from([task("undeclared")]);
    let home = crate::issue::storage::snapshot(input);
    let id = crate::IssueId(String::from(TASK));
    assert!(home.observation(folder(), &id).is_some());
}

#[test]
fn inaccessible_and_deleted_records_keep_their_last_known_content()
-> Result<(), String> {
    use crate::issue::{IssueState, Observation, Unreadable};
    let home = crate::issue::storage::snapshot(inputs(Ok(FLOW)));
    let id = crate::IssueId(String::from(TASK));
    let record = Unreadable {
        id: id.clone(),
        project: home
            .project(FOLDER)
            .ok_or_else(|| String::from("fixture project"))?
            .id,
        title: Some(String::from("Last seen title")),
        body: Some(String::from("Last seen body")),
        created: Some(UnixMillis(100)),
        reason: String::from("access is unavailable"),
    };
    let mut unavailable = Observation::invalid(record.clone());
    unavailable.state = IssueState::Unavailable(record.clone());
    unavailable.observed_at = Some(UnixMillis(250));
    let snapshot = home.with(unavailable.clone());
    assert_eq!(
        snapshot
            .observation(folder(), &id)
            .map(|record| (&record.state, record.observed_at)),
        Some((&unavailable.state, Some(UnixMillis(250))))
    );
    assert!(
        snapshot
            .observation(folder(), &id)
            .is_some_and(|record| !record.flow_findings.is_empty())
    );
    assert!(snapshot.issue(folder(), &id).is_none());
    assert_eq!(snapshot.issues().values().flatten().count(), 1);
    let mut deleted = unavailable;
    deleted.state = IssueState::Deleted(Unreadable {
        reason: String::from("deletion confirmed"),
        ..record
    });
    let confirmed = snapshot.with(deleted);
    assert_eq!(confirmed.graph().problems, Vec::new());
    assert!(
        matches!(confirmed.observation(folder(), &id).map(|observation| &observation.state), Some(IssueState::Deleted(record)) if record.title.as_deref() == Some("Last seen title"))
    );
    Ok(())
}

#[test]
fn locators_cannot_change_identity() -> Result<(), String> {
    use crate::issue::{Locator, LocatorConflict};
    let home = crate::issue::storage::snapshot(inputs(Ok(FLOW)));
    let identity = crate::IssueId(String::from(TASK));
    let observed = home
        .observation(folder(), &identity)
        .ok_or_else(|| String::from("fixture issue"))?;
    let locator = Locator {
        repository: String::from("repo-one"),
        url: None,
        addresses: BTreeMap::new(),
    };
    let configured = [String::from("repo-one"), String::from("repo-two")];
    let located = observed
        .relocated(&identity, locator, &configured)
        .map_err(|error| format!("{error:?}"))?;
    let refreshed = crate::issue::Locator {
        repository: String::from("repo-two"),
        url: None,
        addresses: BTreeMap::new(),
    };
    let renamed = located
        .relocated(&identity, refreshed.clone(), &configured)
        .map_err(|error| format!("{error:?}"))?;
    assert_eq!(
        renamed.issue().map(|issue| &issue.branch),
        located.issue().map(|issue| &issue.branch)
    );
    let other = crate::IssueId(String::from("rest-42"));
    assert_eq!(
        located.relocated(&other, refreshed.clone(), &configured),
        Err(LocatorConflict::ChangedIdentity)
    );
    assert_eq!(
        located.relocated(&identity, refreshed, &[]),
        Err(LocatorConflict::UnconfiguredRepository)
    );
    Ok(())
}

#[test]
fn flow_invalid_values_and_findings_remain_with_the_observation()
-> Result<(), String> {
    let home = crate::issue::storage::snapshot(inputs(Ok(FLOW)));
    let mut observation = decode(home.flow()?, &task("open"))
        .map_err(|problem| problem.to_string())?
        .0;
    let valid = home.with(observation.clone());
    assert!(
        valid
            .observation(observation.project(), observation.id())
            .is_some_and(|record| !record.flow_findings.is_empty())
    );
    if let crate::issue::IssueState::Valid { issue, .. } =
        &mut observation.state
    {
        issue.status = String::from("undeclared");
    }
    let invalid = home.with(observation.clone());
    assert!(
        matches!(invalid.observation(observation.project(), observation.id()).map(|record| &record.state), Some(crate::issue::IssueState::Invalid(record)) if record.title.as_deref() == Some("Write it"))
    );
    Ok(())
}

#[test]
fn mapped_human_notes_never_become_a_stage_move() -> Result<(), String> {
    use crate::issue::{Observation, body::Body};
    let home = crate::issue::storage::snapshot(inputs(Ok(FLOW)));
    let original = decode(home.flow()?, &task("open"))
        .map_err(|problem| problem.to_string())?
        .0;
    let mut issue = original.editable()?.0.clone();
    issue.notes.push(crate::issue::Note {
        id: String::from("comment-1"),
        author: String::from("human"),
        created: UnixMillis(50),
        text: String::from(
            "2026-11-25T00:00:00Z human stage shape→build: signed off",
        ),
    });
    let mapped = Observation::mapped(
        issue,
        Body::of(
            "Description.\n\n## Notes\n- 2026-11-25T00:00:00Z human stage shape→build: signed off",
        ),
    );
    assert_eq!(mapped.editable()?.0.log, Vec::new());
    assert_eq!(mapped.editable()?.0.notes.len(), 1);
    assert_eq!(mapped.editable()?.0.body, "Description.");
    let encoded = serde_json::to_string(mapped.editable()?.1)
        .map_err(|error| error.to_string())?;
    assert!(encoded.starts_with('"'));
    assert!(!encoded.contains("rest"));
    Ok(())
}
