use super::*;
use alloc::collections::BTreeMap;
use home::UnixMillis;
use home::issue::Coverage;

fn record() -> Result<Record, String> {
    Ok(Record {
        id: IssueId::new("42").ok_or("missing identity")?,
        locator: Locator {
            repository: String::from("owner/repo"),
            url: None,
            addresses: BTreeMap::new(),
        },
        title: String::from("Edited in GitHub"),
        body: String::from(
            "## What\nRead this\n\n## Checklist\n- [x] Finished",
        ),
        created: UnixMillis(1000),
        open: true,
        state_reason: None,
        labels: Vec::from([String::from("Customer report")]),
    })
}

fn read(input: Record, options: &str) -> Result<Observation, String> {
    let flow = home::flow::load(include_str!(
        "../../../home/src/flow/test-flow.toml"
    ))?;
    let config = crate::configuration::load(options, &flow)?;
    Ok(observe(&flow, &config, ProjectId::new(71), input))
}

#[test]
fn a_direct_issue_uses_declared_defaults_and_native_fields()
-> Result<(), String> {
    let observation = read(record()?, "")?;
    let (issue, _) = observation.editable()?;
    assert_eq!(issue.id.as_str(), "42");
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
    assert!(matches!(
        observation.completeness.relations.get("blocked_by"),
        Some(Coverage::Incomplete { .. })
    ));
    assert!(observation.revision.is_some());
    Ok(())
}

fn with_labels(input: &mut Record, labels: &[&str]) {
    input.labels = labels.iter().copied().map(String::from).collect();
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
    assert_eq!(problem.id.as_str(), "42");
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
        input.open = open;
        input.state_reason = reason.map(String::from);
        assert_eq!(read(input, options)?.editable()?.0.status, expected);
    }
    let mut unknown = record()?;
    unknown.open = false;
    unknown.state_reason = Some(String::from("duplicate"));
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
    input.open = false;
    input.state_reason = Some(String::from("completed"));
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
    input.body = String::from(
        "## What\nAn example\n\n````text\n```project-data\nexample only\n```\n````\n\n## Checklist\n- [ ] inspect\n\n~~~project-data\nschema = 1\n[fields]\nbranch='decision-work'\nmethod='prototype'\n[relations]\nparent='712'\n~~~\n\n",
    );
    with_labels(&mut input, &["work/kind/decision"]);
    let observed =
        read(input, "[[relations]]\nname='parent'\nsource='metadata'")?;
    let (issue, body) = observed.editable()?;
    assert_eq!(issue.branch, "decision-work");
    assert_eq!(issue.text("method"), Some("prototype"));
    assert_eq!(
        issue.links.get("parent").and_then(|ids| ids.first()),
        IssueId::new("712").as_ref()
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
fn invalid_footer_data_and_competing_native_sources_are_visible()
-> Result<(), String> {
    for data in [
        "schema=2",
        "malformed toml",
        "schema=1\n[fields]\npriority='high'",
        "schema=1\n[fields]\nbranch=true",
        "schema=1\n[fields]\nmethod='prototype'",
        "schema=1\n[fields]\nunknown='word'",
        "schema=1\n[relations]\nparent='123'",
        "schema=1\noperation='not a handle'",
    ] {
        let mut input = record()?;
        input.body = format!("Read me\n\n```project-data\n{data}\n```");
        let observed = read(input, "")?;
        let home::issue::IssueState::Invalid(unreadable) = observed.state
        else {
            return Err(format!("accepted {data:?}"));
        };
        assert_eq!(
            unreadable.body.as_deref().map(str::trim_end),
            Some("Read me")
        );
        assert_eq!(unreadable.id.as_str(), "42");
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
        input.body = format!("Read me\n\n{tail}");
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
    input.body = String::from(
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
    input.labels.reverse();
    assert_eq!(read(input.clone(), "")?.revision, original);
    input.body.push_str("\nEdited in GitHub");
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
    input.body = format!(
        "```project-data\nschema=1\noperation='{marker}'\n[fields]\nflagged=true\nusers=['one','two']\ndue=2026-01-01T12:20:40Z\n```"
    );
    let observed = observe(&flow, &config, ProjectId::new(1), input);
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
    for value in ["''", "42", "['42', '43']", "['42']"] {
        let mut input = record()?;
        input.body = format!(
            "```project-data\nschema=1\n[relations]\nparent={value}\n```"
        );
        let observed =
            read(input, "[[relations]]\nname='parent'\nsource='metadata'")?;
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
