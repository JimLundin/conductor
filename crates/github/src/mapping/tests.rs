use super::*;
use alloc::collections::BTreeMap;
use home::issue::Coverage;
use home::{IssueKey, UnixMillis};

fn record() -> Result<Record, String> {
    Ok(Record {
        id: IssueId::of(
            &crate::IssueNumber::new(42).ok_or("missing identity")?,
        ),
        locator: Locator {
            repository: String::from("owner/repo"),
            url: None,
            addresses: BTreeMap::new(),
        },
        created: UnixMillis(1000),
        native: home::issue::NativeIssue {
            title: String::from("Edited in GitHub"),
            body: String::from(
                "## What\nRead this\n\n## Checklist\n- [x] Finished",
            ),
            open: true,
            reason: None,
            labels: Vec::from([String::from("Customer report")]),
        },
        parent: None,
        blocked_by: Some(Vec::new()),
    })
}

fn ids() -> Ids {
    Ids::default()
        .with(ProjectId::new(71), crate::ISSUER)
        .with(ProjectId::new(1), crate::ISSUER)
}

fn read(input: Record, options: &str) -> Result<Observation, String> {
    let flow = home::flow::load(include_str!(
        "../../../home/src/flow/test-flow.toml"
    ))?;
    let config = crate::configuration::load(options, &flow)?;
    Ok(observe(&flow, &config, &ids(), ProjectId::new(71), input))
}

#[test]
fn a_direct_issue_uses_declared_defaults_and_native_fields()
-> Result<(), String> {
    let observation = read(record()?, "")?;
    let (issue, _) = observation.editable()?;
    assert_eq!(issue.id.to_string(), "42");
    assert_eq!(issue.title, "Edited in GitHub");
    assert_eq!(issue.body, "## What\nRead this");
    assert!(
        issue
            .checklist
            .first()
            .is_some_and(|item| item.done && item.text == "Finished")
    );
    assert_eq!(issue.created, Some(UnixMillis(1000)));
    assert_eq!(issue.text("created"), Some("1970-01-01T00:00:01Z"));
    assert_eq!(issue.kind, "task");
    assert_eq!(issue.status, "open");
    assert_eq!(issue.stage, None);
    assert_eq!(issue.text("priority"), Some("normal"));
    assert_eq!(issue.texts("labels"), ["Customer report"]);
    assert!(matches!(
        observation.completeness.notes,
        Coverage::Incomplete { .. }
    ));
    assert_eq!(
        observation.completeness.relations.get("blocked_by"),
        Some(&Coverage::Complete)
    );
    assert!(observation.revision.is_some());
    Ok(())
}

fn with_labels(input: &mut Record, labels: &[&str]) {
    input.native.labels = labels.iter().copied().map(String::from).collect();
}

#[test]
fn managed_labels_follow_the_mapping_and_leave_ordinary_labels_alone()
-> Result<(), String> {
    let mut input = record()?;
    with_labels(
        &mut input,
        &[
            "Work/Kind/Spec",
            "WORK/stage/build",
            "work/status/ready",
            "work/priority/high",
            "bug",
        ],
    );
    let observed = read(input, "")?;
    let (issue, _) = observed.editable()?;
    assert_eq!(issue.kind, "spec");
    assert_eq!(issue.stage.as_deref(), Some("build"));
    assert_eq!(issue.status, "ready");
    assert_eq!(issue.text("priority"), Some("high"));
    assert_eq!(issue.texts("labels"), ["bug"]);
    Ok(())
}

#[test]
fn unknown_and_ambiguous_managed_labels_leave_the_issue_visible()
-> Result<(), String> {
    let mut input = record()?;
    with_labels(
        &mut input,
        &["Work/unknown/v", "work/priority/high", "work/priority/low"],
    );
    let observed = read(input, "")?;
    let home::issue::IssueState::Invalid(problem) = observed.state else {
        return Err(String::from("invalid issue expected"));
    };
    assert_eq!(problem.title.as_deref(), Some("Edited in GitHub"));
    assert!(
        problem
            .body
            .as_deref()
            .is_some_and(|body| body.contains("## What"))
    );
    assert_eq!(problem.id.to_string(), "42");
    assert!(
        observed
            .findings
            .iter()
            .any(|f| f.message.contains("Work/unknown/v"))
    );
    assert!(
        observed
            .findings
            .iter()
            .any(|f| f.message.contains("priority"))
    );
    Ok(())
}

#[test]
fn native_close_and_reopen_use_only_declared_defaults() -> Result<(), String> {
    let options = "[defaults]\nopen='ready'\ncompleted='done'\nnot_planned='dropped'\nclosed='dropped'";
    for (open, reason, expected) in [
        (true, None, "ready"),
        (false, None, "dropped"),
        (false, Some("completed"), "done"),
        (false, Some("not_planned"), "dropped"),
        (true, Some("reopened"), "ready"),
    ] {
        let mut input = record()?;
        input.native.open = open;
        input.native.reason = reason.map(String::from);
        assert_eq!(read(input, options)?.editable()?.0.status, expected);
    }
    let mut unknown = record()?;
    unknown.native.open = false;
    unknown.native.reason = Some(String::from("duplicate"));
    assert!(read(unknown, options)?.editable().is_err());
    Ok(())
}

#[test]
fn explicit_status_requires_matching_open_state_but_native_reason_is_advisory()
-> Result<(), String> {
    let mut input = record()?;
    with_labels(&mut input, &["work/status/dropped"]);
    let conflict = read(input.clone(), "")?;
    assert!(conflict.editable().is_err());
    assert!(conflict.findings.iter().any(|f| f.message.contains("open")));
    input.native.open = false;
    input.native.reason = Some(String::from("completed"));
    let observed = read(input, "")?;
    assert_eq!(observed.editable()?.0.status, "dropped");
    assert!(
        observed
            .findings
            .iter()
            .any(|f| f.severity == home::flow::Severity::Warn
                && f.message.contains("completed"))
    );
    Ok(())
}

#[test]
fn one_final_footer_supplies_typed_fields_and_relations_by_issue_id_outside_the_body()
-> Result<(), String> {
    let mut input = record()?;
    input.native.body = String::from(
        "## What\nAn example\n\n````text\n```project-data\nexample only\n```\n````\n\n## Checklist\n- [ ] inspect\n\n~~~project-data\nschema = 1\n[fields]\nbranch='decision-work'\nmethod='prototype'\n[relations]\nparent='712'\n~~~\n\n",
    );
    with_labels(&mut input, &["work/kind/decision"]);
    let observed = read(input, "")?;
    let (issue, body) = observed.editable()?;
    assert_eq!(issue.branch, "decision-work");
    assert_eq!(issue.text("method"), Some("prototype"));
    assert_eq!(
        issue.links.get("parent").and_then(|ids| ids.first()),
        crate::IssueNumber::new(712)
            .map(|number| IssueKey {
                project: ProjectId::new(71),
                id: IssueId::of(&number),
            })
            .as_ref()
    );
    assert!(body.text().contains("example only"));
    assert!(!body.text().contains("schema = 1"));
    assert!(
        issue
            .checklist
            .first()
            .is_some_and(|item| !item.done && item.text == "inspect")
    );
    assert_eq!(
        observed.completeness.relations.get("parent"),
        Some(&Coverage::Complete)
    );
    Ok(())
}

#[test]
fn a_footer_relation_names_another_project_s_issue_by_its_project_and_id()
-> Result<(), String> {
    let mut input = record()?;
    input.native.body = String::from(
        "Read me\n\n```project-data\nschema = 1\n[relations]\nparent = { project = 'hyn162', id = '0trfecpc' }\n```",
    );
    let observed = read(input, "")?;
    let (issue, _) = observed.editable()?;
    assert_eq!(
        issue
            .links
            .get("parent")
            .and_then(|targets| targets.first())
            .map(|target| (target.project.to_string(), target.id.to_string())),
        Some((String::from("hyn162"), String::from("0trfecpc")))
    );
    Ok(())
}

#[test]
fn invalid_footer_data_is_visible() -> Result<(), String> {
    for data in [
        "schema=2",
        "malformed toml",
        "schema=1\n[fields]\npriority='high'",
        "schema=1\n[fields]\nbranch=true",
        "schema=1\n[fields]\nmethod='prototype'",
        "schema=1\n[fields]\nunknown='word'",
        "schema=1\n[relations]\nunknown='123'",
        "schema=1\n[relations]\nchildren='123'",
        "schema=1\noperation='not a handle'",
    ] {
        let mut input = record()?;
        input.native.body = format!("Read me\n\n```project-data\n{data}\n```");
        let observed = read(input, "")?;
        let home::issue::IssueState::Invalid(unreadable) = observed.state
        else {
            return Err(format!("accepted {data:?}"));
        };
        assert_eq!(
            unreadable.body.as_deref().map(str::trim_end),
            Some("Read me")
        );
        assert_eq!(unreadable.id.to_string(), "42");
        assert!(
            observed
                .findings
                .iter()
                .any(|finding| finding.severity == Severity::Error)
        );
    }
    Ok(())
}

#[test]
fn a_reserved_block_must_be_single_closed_and_final() -> Result<(), String> {
    for tail in [
        "```project-data\nschema=1",
        "```project-data\nschema=1\n```\nHuman text afterward",
        "```project-data\nschema=1\n```\n```project-data\nschema=1\n```",
    ] {
        let mut input = record()?;
        input.native.body = format!("Read me\n\n{tail}");
        let observation = read(input, "")?;
        let home::issue::IssueState::Invalid(problem) = observation.state
        else {
            return Err(format!("accepted {tail:?}"));
        };
        assert!(
            problem
                .body
                .as_deref()
                .is_some_and(|text| text.contains("Read me"))
        );
    }
    Ok(())
}

#[test]
fn a_human_notes_heading_cannot_supply_approval_or_native_comments()
-> Result<(), String> {
    let mut input = record()?;
    input.native.body = String::from(
        "## Notes\n- 2026-11-25T06:00Z me stage build→land: signed off",
    );
    let observed = read(input, "")?;
    let (issue, body) = observed.editable()?;
    assert_eq!(issue.log, []);
    assert_eq!(issue.notes, []);
    assert!(body.text().contains("signed off"));
    Ok(())
}

#[test]
fn fingerprints_follow_observed_fields_and_ignore_label_order()
-> Result<(), String> {
    let mut input = record()?;
    with_labels(&mut input, &["bug", "work/priority/high"]);
    let original = read(input.clone(), "")?.revision;
    input.native.labels.reverse();
    assert_eq!(read(input.clone(), "")?.revision, original);
    input.native.body.push_str("\nEdited in GitHub");
    assert_ne!(read(input, "")?.revision, original);
    Ok(())
}

#[test]
fn footer_values_keep_declared_types_and_an_owner_bound_create_marker()
-> Result<(), String> {
    let flow = home::flow::load(&include_str!("../../../home/src/flow/test-flow.toml").replacen(
        "[[kind]]",
        "[[field]]\nname='flagged'\ntype='bool'\n[[field]]\nname='users'\ntype='texts'\n[[field]]\nname='due'\ntype='time'\n[[kind]]",
        1,
    ))?;
    let config = crate::configuration::load("", &flow)?;
    let mut input = record()?;
    let marker = home::operations::Handle {
        owner: [0xAB; 32],
        id: home::issue::EditId(3),
    };
    input.native.body = format!(
        "```project-data\nschema=1\noperation='{marker}'\n[fields]\nflagged=true\nusers=['one','two']\ndue=2026-01-01T12:20:40Z\n```"
    );
    let observed = observe(&flow, &config, &ids(), ProjectId::new(1), input);
    let (issue, _) = observed.editable()?;
    assert_eq!(issue.fields.get("flagged"), Some(&Datum::Bool(true)));
    assert_eq!(issue.texts("users"), ["one", "two"]);
    assert_eq!(issue.text("due"), Some("2026-01-01T12:20:40Z"));
    assert_eq!(issue.body, "");
    Ok(())
}

#[test]
fn metadata_edges_require_issue_ids_and_declared_cardinality()
-> Result<(), String> {
    for value in [
        "''",
        "42",
        "['42', '43']",
        "['42']",
        "'team42/7'",
        "'0tm7aaaa'",
    ] {
        let mut input = record()?;
        input.native.body = format!(
            "```project-data\nschema=1\n[relations]\nparent={value}\n```"
        );
        let observed = read(input, "")?;
        assert!(observed.editable().is_err(), "{value}");
    }
    Ok(())
}

#[test]
fn creation_time_keeps_native_subseconds_in_the_declared_field()
-> Result<(), String> {
    let mut input = record()?;
    input.created = UnixMillis(1090);
    let observed = read(input, "")?;
    let (issue, _) = observed.editable()?;
    assert_eq!(issue.created, Some(UnixMillis(1090)));
    assert_eq!(issue.text("created"), Some("1970-01-01T00:00:01.090Z"));
    Ok(())
}

#[test]
fn a_native_parent_and_a_different_footer_parent_conflict() -> Result<(), String>
{
    let mut input = record()?;
    input.parent = Some(Native::Here(3));
    input.native.body = String::from(
        "Read me\n\n```project-data\nschema = 1\n[relations]\nparent = '4'\n```",
    );
    assert!(read(input.clone(), "")?.editable().is_err());
    input.native.body = String::from(
        "Read me\n\n```project-data\nschema = 1\n[relations]\nparent = '3'\n```",
    );
    let observed = read(input, "")?;
    assert_eq!(
        observed.editable()?.0.links.get("parent"),
        crate::IssueNumber::new(3)
            .map(|number| Vec::from([IssueKey {
                project: ProjectId::new(71),
                id: IssueId::of(&number),
            }]))
            .as_ref()
    );
    Ok(())
}

#[test]
fn unread_dependencies_leave_blocked_by_incomplete() -> Result<(), String> {
    let mut input = record()?;
    input.blocked_by = None;
    let observed = read(input, "")?;
    assert!(matches!(
        observed.completeness.relations.get("blocked_by"),
        Some(Coverage::Incomplete { .. })
    ));
    assert_eq!(
        observed.completeness.relations.get("parent"),
        Some(&Coverage::Complete)
    );
    Ok(())
}

#[test]
fn a_footer_relation_write_keeps_the_prose_the_fields_and_the_other_relations()
-> Result<(), String> {
    let flow = home::flow::load(include_str!(
        "../../../home/src/flow/test-flow.toml"
    ))?;
    let mut input = record()?;
    input.native.body = String::from(
        "Read me\n\n```project-data\nschema = 1\n[fields]\nbranch = 'kept'\n[relations]\nparent = { project = 'hyn162', id = '0trfecpc' }\n```\n",
    );
    let here = ProjectId::new(71);
    let hyn162 = ProjectId::parse("hyn162").ok_or("project")?;
    let elsewhere = IssueKey {
        project: hyn162,
        id: Ids::default().read(hyn162, "0trfecpc")?,
    };
    let kept = IssueKey {
        project: hyn162,
        id: Ids::default().read(hyn162, "0tm7aaaa")?,
    };
    let plan = relate::plan(
        &flow,
        &ids(),
        here,
        &input,
        &home::issue::RelationChange {
            relation: String::from("blocked_by"),
            targets: Vec::from([kept.clone()]),
        },
    )
    .map_err(|error| error.to_string())?;
    assert_eq!(plan.changes, []);
    let step = plan.footer.ok_or("no footer step")?;
    let Some(relate::Request::Body(body)) =
        relate::Request::decode(&step.request)
    else {
        return Err(String::from("not a body write"));
    };
    assert!(
        body.contains(
            "[[relations.blocked_by]]\nid = \"0tm7aaaa\"\nproject = \"hyn162\"\n"
        ),
        "{body}"
    );
    input.native.body = body;
    let observed = read(input, "")?;
    let (issue, prose) = observed.editable()?;
    assert_eq!(prose.text().trim(), "Read me");
    assert_eq!(issue.branch, "kept");
    assert_eq!(issue.links.get("parent"), Some(&Vec::from([elsewhere])));
    assert_eq!(issue.links.get("blocked_by"), Some(&Vec::from([kept])));
    Ok(())
}

fn planned(
    native: &home::issue::NativeIssue,
    mutations: &[home::flow::command::Mutation],
) -> Result<Vec<home::issue::IssueWrite>, String> {
    let flow = home::flow::load(include_str!(
        "../../../home/src/flow/test-flow.toml"
    ))?;
    let config = crate::configuration::load("", &flow)?;
    writes(&flow, &config, native, mutations)
}

fn labelled(labels: &[&str]) -> Result<home::issue::NativeIssue, String> {
    let mut native = record()?.native;
    native.labels = labels.iter().copied().map(String::from).collect();
    Ok(native)
}

fn moved(part: home::issue::Part, to: &str) -> home::flow::command::Mutation {
    home::flow::command::Mutation::Move {
        part,
        to: String::from(to),
    }
}

fn logged() -> home::flow::command::Mutation {
    home::flow::command::Mutation::Note(home::issue::Entry {
        at: None,
        by: None,
        event: home::issue::LogEvent::Note,
        text: String::from("moved"),
    })
}

#[test]
fn a_stage_move_adds_the_new_stage_label_then_removes_the_old_one_and_touches_nothing_else()
-> Result<(), String> {
    use home::issue::IssueWrite;
    let native = labelled(&["Customer report", "work/stage/shape"])?;
    assert_eq!(
        planned(
            &native,
            &[moved(home::issue::Part::Stage, "spec"), logged()]
        )?,
        [
            IssueWrite::AddLabel(String::from("work/stage/spec")),
            IssueWrite::RemoveLabel(String::from("work/stage/shape")),
        ]
    );
    Ok(())
}

#[test]
fn a_move_to_a_terminal_status_swaps_its_label_and_closes_the_issue_as_completed()
-> Result<(), String> {
    use home::issue::IssueWrite;
    let native = labelled(&["work/status/active"])?;
    assert_eq!(
        planned(&native, &[moved(home::issue::Part::Status, "done")])?,
        [
            IssueWrite::AddLabel(String::from("work/status/done")),
            IssueWrite::RemoveLabel(String::from("work/status/active")),
            IssueWrite::Close {
                reason: String::from("completed"),
                was: None,
            },
        ]
    );
    Ok(())
}

#[test]
fn a_body_edit_replaces_the_human_text_and_keeps_the_project_data_footer()
-> Result<(), String> {
    let footer =
        "```project-data\nschema = 1\n\n[fields]\nbranch = \"kept\"\n```\n";
    let mut native = record()?.native;
    native.body = format!("Old words\n\n{footer}");
    let written = planned(
        &native,
        &[home::flow::command::Mutation::Body(String::from(
            "New words",
        ))],
    )?;
    assert_eq!(
        written,
        [home::issue::IssueWrite::Body {
            from: native.body,
            to: format!("New words\n\n{footer}"),
        }]
    );
    Ok(())
}

#[test]
fn setting_a_metadata_field_changes_only_its_footer_entry() -> Result<(), String>
{
    let mut native = record()?.native;
    native.body = String::from(
        "Words\n\n```project-data\nschema = 1\n\n[fields]\nmethod = \"shape\"\n```\n",
    );
    let written = planned(
        &native,
        &[home::flow::command::Mutation::Set {
            field: String::from("branch"),
            value: Datum::Text(String::from("edits")),
        }],
    )?;
    assert_eq!(
        written,
        [home::issue::IssueWrite::Body {
            from: native.body,
            to: String::from(
                "Words\n\n```project-data\nschema = 1\n\n[fields]\nbranch = \"edits\"\nmethod = \"shape\"\n```\n",
            ),
        }]
    );
    Ok(())
}

#[test]
fn a_relation_or_a_lone_note_plans_no_issue_write() -> Result<(), String> {
    let native = record()?.native;
    let relation = planned(
        &native,
        &[home::flow::command::Mutation::Relate {
            relation: String::from("blocked_by"),
            many: true,
            targets: Vec::from([IssueKey {
                project: ProjectId::new(71),
                id: IssueId::of(&crate::IssueNumber::new(7).ok_or("id")?),
            }]),
        }],
    )?;
    assert_eq!(relation, []);
    assert_eq!(planned(&native, &[logged()])?, []);
    Ok(())
}

#[test]
fn opening_an_issue_labels_its_kind_and_stage_and_marks_its_footer_with_the_operation()
-> Result<(), String> {
    use home::flow::command::{Change, Mutation};
    use home::issue::{Entry, LogEvent, body::Body};
    let flow = home::flow::load(include_str!(
        "../../../home/src/flow/test-flow.toml"
    ))?;
    let config = crate::configuration::load("", &flow)?;
    let opened = Entry {
        at: Some(String::from("2026-11-28T10:00:00Z")),
        by: Some(String::from("devbox·claude")),
        event: LogEvent::Note,
        text: String::from("opened."),
    };
    let issue = home::issue::Issue {
        id: IssueId::of(&crate::IssueNumber::new(7).ok_or("id")?),
        project: ProjectId::new(1),
        created: Some(UnixMillis(1000)),
        title: String::from("Open me on GitHub"),
        kind: String::from("spec"),
        status: String::from("open"),
        stage: Some(String::from("shape")),
        fields: BTreeMap::new(),
        links: BTreeMap::new(),
        body: String::new(),
        sections: Vec::new(),
        checklist: Vec::new(),
        log: Vec::new(),
        notes: Vec::new(),
        branch: String::new(),
        canvases: Vec::new(),
    };
    let set = |field: &str, value: &str| Mutation::Set {
        field: String::from(field),
        value: Datum::Text(String::from(value)),
    };
    let change = Change {
        observation: Observation::mapped(
            issue,
            Body::new(&[String::from("Problem")], &opened),
        ),
        created: true,
        mutations: Vec::from([
            set("stage", "shape"),
            set("branch", "opened"),
            set("created", "2026-11-28T10:00:00Z"),
            Mutation::Note(opened),
        ]),
    };
    let create = opening(&flow, &config, &change, "ab/cd")?;
    assert_eq!(create.title, "Open me on GitHub");
    assert_eq!(create.labels, ["work/kind/spec", "work/stage/shape"]);
    assert_eq!(
        create.body,
        "## Problem\n\n```project-data\noperation = \"ab/cd\"\nschema = 1\n\n[fields]\nbranch = \"opened\"\n```\n"
    );
    Ok(())
}
