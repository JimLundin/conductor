use std::path::Path;

use home::IssueId;
use home::flow::snapshot::{Inputs, Snapshot};
use home::project::Project;

use super::{ISSUERS, connect};

fn home(adapter: &str) -> Snapshot {
    let project = format!(
        "name = \"tracked\"\ntitle = \"Tracked\"\nroot = \"fc27e50efb661d7ef4a45d3edbe0234ff0a6fbd5\"\n[backends.tracker]\nadapter = \"{adapter}\"\nresource = \"owner/issues\"\n"
    );
    Snapshot::of(Inputs {
        flow: home::flow::load(include_str!(
            "../../home/src/flow/test-flow.toml"
        )),
        projects: home::project::listed(vec![(
            String::from("a1b2c3"),
            Ok(project),
        )]),
        issues: Vec::new(),
        unread: Vec::new(),
        issuers: ISSUERS,
    })
}

fn tracked(home: &Snapshot) -> Result<&Project, String> {
    home.project("tracked")
        .ok_or_else(|| String::from("no project is called tracked"))
}

#[test]
fn a_github_project_reads_issue_numbers_as_its_ids() -> Result<(), String> {
    let home = home("github");
    let project = tracked(&home)?.id;
    assert_eq!(
        home.ids().read(project, "146").ok(),
        Some(IssueId::of(
            &github::IssueNumber::new(146).ok_or("no issue number")?
        ))
    );
    assert!(home.ids().read(project, "0tm7aaaa").is_err());
    Ok(())
}

#[test]
fn a_tracker_of_an_adapter_no_table_row_names_is_refused_by_its_name()
-> Result<(), String> {
    let home = home("linear");
    let refused = connect(Path::new("."), &home, tracked(&home)?, |_, _| {
        Ok(project_runtime::transport::SharedCredential::Standalone)
    })
    .err();
    assert_eq!(
        refused,
        Some(String::from("no adapter is called \"linear\""))
    );
    Ok(())
}
