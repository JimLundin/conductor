use proto::ProjectId;

use proto::flow::Severity;

use super::{Finding, check as checked};
use crate::flow::graph::File;
use crate::flow::snapshot::{Inputs, Snapshot};

const FLOW: &str = include_str!("../test-flow.toml");
const ROOT: &str = "e22b7f7b2976650400965d4147ba771675f96076";
const SPEC: &str = "0tm7zt00";
const REVIEW: &str = "0tm7zx0c";

fn project(
    folder: &str,
    name: &str,
    provides: &str,
) -> (String, Result<String, String>) {
    (
        String::from(folder),
        Ok(format!(
            "name = \"{name}\"\ntitle = \"{name}\"\nroot = \"{ROOT}\"\nprovides = [{provides}]\n"
        )),
    )
}

fn issue(id: &str, front: &str, body: &str) -> File {
    File {
        project: ProjectId::parse("7r5n23").unwrap_or(ProjectId::new(0)),
        stem: String::from(id),
        text: format!("+++\nid = \"{id}\"\n{front}+++\n\n{body}"),
    }
}

fn inputs(flow: &str, review_status: &str) -> Inputs {
    Inputs {
        flow: Ok(String::from(flow)),
        projects: Vec::from([project("7r5n23", "conductor", "")]),
        issues: Vec::from([
            issue(
                SPEC,
                "title = \"Interpret the flow\"\nkind = \"spec\"\nstatus = \"done\"\n",
                "## Problem\n\n## Intended design\n\n## Acceptance\n",
            ),
            issue(
                REVIEW,
                &format!(
                    "title = \"Review\"\nkind = \"review\"\nstatus = \"{review_status}\"\n\
                     focus = \"design\"\nparent = \"{SPEC}\"\n"
                ),
                "## Scope\n\n## Findings\n",
            ),
        ]),
        unread: Vec::new(),
    }
}

fn check(inputs: &Inputs) -> super::Report {
    checked(&Snapshot::of(inputs.clone()))
}

fn subject(id: &str) -> String {
    format!("conductor/{id}")
}

#[test]
fn a_broken_rule_reports_at_its_severity_with_its_explanation() {
    let report = check(&inputs(FLOW, "active"));
    assert_eq!(
        report.findings,
        Vec::from([
            Finding {
                severity: Severity::Warn,
                subject: Some(subject(SPEC)),
                message: format!(
                    "A spec is done only after a design review: {{ status_role = \"terminal\" }} \
                     fails on {REVIEW} (via children {REVIEW})"
                ),
            },
            Finding {
                severity: Severity::Warn,
                subject: Some(subject(REVIEW)),
                message: String::from("lacks the section \"Verdict\""),
            },
        ])
    );
    assert!(!report.failed());
    let strict = FLOW.replacen(
        "message = \"A spec is done only after a design review\"\nseverity = \"warn\"",
        "message = \"A spec is done only after a design review\"\nseverity = \"error\"",
        1,
    );
    assert!(check(&inputs(&strict, "active")).failed());
    assert!(!check(&inputs(&strict, "done")).failed());
}

#[test]
fn structural_problems_fail_the_check() {
    let mut broken = inputs(FLOW, "in-progress");
    broken
        .projects
        .push(project("m800gj", "standards", "\"rust\""));
    broken.projects.push(project("m80000", "other", "\"rust\""));
    let report = check(&broken);
    assert!(report.failed());
    assert_eq!(
        report
            .findings
            .iter()
            .filter(|finding| finding.severity == Severity::Error)
            .map(|finding| (finding.subject.clone(), finding.message.clone()))
            .collect::<Vec<_>>(),
        Vec::from([
            (
                None,
                String::from(
                    "profile \"rust\" is provided by more than one project: other, standards"
                ),
            ),
            (
                Some(subject(REVIEW)),
                String::from("status: no status is called \"in-progress\"")
            ),
        ])
    );
}

#[test]
fn a_flow_that_does_not_load_is_the_only_finding() {
    let report = check(&inputs("default_kind = \"chore\"\n", "done"));
    assert!(report.failed());
    assert_eq!(report.findings.len(), 1);
    assert!(report.findings.first().is_some_and(|finding| {
        finding
            .message
            .starts_with("docs/agents/flow.toml: flow: no status")
    }));
}

#[test]
fn the_text_lists_each_finding_and_the_counts() {
    assert_eq!(
        check(&inputs(FLOW, "done")).text(),
        format!(
            "warn   conductor/{REVIEW}  lacks the section \"Verdict\"\n0 errors, 1 warnings\n"
        )
    );
}

#[test]
fn two_issue_files_with_one_id_fail_the_check() {
    let mut home = inputs(FLOW, "done");
    home.projects.push(project("8p0d1x", "other", ""));
    home.issues.push(File {
        project: ProjectId::parse("8p0d1x").unwrap_or(ProjectId::new(0)),
        ..issue(
            SPEC,
            "title = \"Again\"\nkind = \"spec\"\nstatus = \"open\"\n",
            "",
        )
    });
    let report = check(&home);
    assert!(report.failed(), "{}", report.text());
    assert!(
        report.findings.contains(&Finding {
            severity: Severity::Error,
            subject: Some(String::from("other/0tm7zt00")),
            message: String::from("id: is also the id of 7r5n23/0tm7zt00"),
        }),
        "{}",
        report.text()
    );
}
