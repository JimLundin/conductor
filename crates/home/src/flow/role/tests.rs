use alloc::collections::BTreeMap;

use crate::issue::Issue;
use crate::{IssueId, ProjectId};

use super::{can_start, landed, opening, started};
use crate::flow::load;
use crate::flow::{Flow, Role, Status};

fn text(value: &str) -> String {
    String::from(value)
}

const FLOW: &str = include_str!("../test-flow.toml");

fn renamed() -> Flow {
    renamed_from(FLOW)
}

fn renamed_from(written: &str) -> Flow {
    let mut flow = load(written).unwrap_or_else(|_| Flow {
        default_kind: String::new(),
        statuses: Vec::new(),
        fields: Vec::new(),
        kinds: Vec::new(),
        relations: Vec::new(),
        stages: Vec::new(),
        rules: Vec::new(),
    });
    for status in &mut flow.statuses {
        status.name = text(match status.role {
            Role::Idle => "backlog",
            Role::Startable => "queued",
            Role::Active => "doing",
            Role::Review => "checking",
            Role::Terminal => "shipped",
            Role::Dropped => "abandoned",
        });
    }
    flow.statuses.push(Status {
        name: text("pairing"),
        role: Role::Active,
        colour: None,
    });
    flow
}

const RENAMED: [&str; 7] = [
    "backlog",
    "queued",
    "doing",
    "checking",
    "shipped",
    "abandoned",
    "pairing",
];

const WAITING: IssueId = IssueId::new(0, 1);
const BLOCKER: IssueId = IssueId::new(0, 2);

fn issue(id: IssueId, status: &str) -> Issue {
    Issue {
        id,
        project: ProjectId::new(1),
        title: text("an issue"),
        kind: text("task"),
        status: text(status),
        stage: None,
        fields: BTreeMap::new(),
        links: BTreeMap::new(),
        body: String::new(),
        sections: Vec::new(),
        checklist: Vec::new(),
        log: Vec::new(),
        branch: String::new(),
        canvases: Vec::new(),
    }
}

#[test]
fn a_new_issue_opens_in_the_first_idle_status() {
    assert_eq!(opening(&renamed()), Some("backlog"));
}

#[test]
fn starting_a_session_makes_an_idle_or_startable_issue_active_by_role() {
    let flow = renamed();
    let after = |status: &str| {
        let issue = issue(WAITING, status);
        started(&flow, &[&issue], &issue)
    };
    assert_eq!(
        RENAMED.map(after),
        [Some("doing"), Some("doing"), None, None, None, None, None]
    );
}

#[test]
fn an_issue_blocked_by_one_not_yet_built_is_not_startable() {
    let flow = renamed();
    let mut waiting = issue(WAITING, "queued");
    waiting
        .links
        .insert(text("blocked_by"), Vec::from([BLOCKER]));
    let offered = |status: &str| {
        let blocking = issue(BLOCKER, status);
        let issues = [&waiting, &blocking];
        (
            can_start(&flow, &issues, &waiting),
            started(&flow, &issues, &waiting).is_some(),
        )
    };
    assert_eq!(
        RENAMED.map(offered),
        [
            (false, false),
            (false, false),
            (false, false),
            (true, true),
            (true, true),
            (true, true),
            (false, false),
        ]
    );
}

#[test]
fn an_issue_not_yet_built_whose_session_landed_work_is_up_for_review_by_role() {
    let flow = renamed();
    assert_eq!(
        RENAMED.map(|status| landed(&flow, &issue(WAITING, status))),
        [
            Some("checking"),
            Some("checking"),
            Some("checking"),
            None,
            None,
            None,
            Some("checking"),
        ]
    );
}

#[test]
fn what_can_start_is_what_the_flow_s_start_rule_says() {
    let blocked_by_one_in = |flow: &Flow, status: &str| {
        let mut waiting = issue(WAITING, "queued");
        waiting
            .links
            .insert(text("blocked_by"), Vec::from([BLOCKER]));
        let blocking = issue(BLOCKER, status);
        can_start(flow, &[&waiting, &blocking], &waiting)
    };
    let built =
        "via = \"blocked_by\", where = { any = [{ status_role = \"review\" }, ";
    assert!(FLOW.contains(built), "the flow's start rule has moved");
    let finished = renamed_from(&FLOW.replacen(
        built,
        "via = \"blocked_by\", where = { any = [",
        1,
    ));
    assert!(blocked_by_one_in(&renamed(), "checking"));
    assert!(!blocked_by_one_in(&finished, "checking"));
    assert!(blocked_by_one_in(&finished, "shipped"));
}

#[test]
fn only_waiting_work_starts() {
    let flow = renamed();
    let from = |status: &str| {
        let issue = issue(WAITING, status);
        can_start(&flow, &[&issue], &issue)
    };
    assert_eq!(
        ["backlog", "queued", "checking", "shipped", "abandoned"].map(from),
        [true, true, false, false, false]
    );
}
