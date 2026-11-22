use proto::ProjectId;

use super::{Inputs, Snapshot};
use crate::flow::graph::File;

const FLOW: &str = include_str!("../test-flow.toml");
const FOLDER: &str = "01m7w85jegh2zxfrxb37dadxnk";
const TASK: &str = "01m7zxfbp2qygtekcr6b6ndg28";

fn task(status: &str) -> File {
    File {
        project: ProjectId(String::from(FOLDER)),
        stem: String::from(TASK),
        text: format!(
            "+++\nid = \"{TASK}\"\ntitle = \"Write it\"\nkind = \"task\"\n\
             status = \"{status}\"\n+++\n"
        ),
    }
}

fn inputs(flow: Result<&str, &str>) -> Inputs {
    Inputs {
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
    let home = Snapshot::of(inputs(Ok(FLOW)));
    let named = |name: &str| {
        home.project(name)
            .map(|project| (project.id.0.as_str(), project.name.as_str()))
    };
    assert_eq!(named("home"), Some((FOLDER, "home")));
    assert_eq!(named(FOLDER), Some((FOLDER, "home")));
    assert_eq!(named("Bad"), None);
    assert_eq!(statuses(&home), ["open"]);
    assert_eq!(
        home.problems(),
        [
            "projects/Bad: the folder's name is not lower-case letters, \
             digits and dashes",
            "projects/x/issues/y.md: denied",
        ]
    );
}

#[test]
fn a_flow_that_does_not_load_reads_no_issues_and_says_where_it_is() {
    let home = Snapshot::of(inputs(Err("not found")));
    assert_eq!(home.flow(), Err("docs/agents/flow.toml: not found"));
    assert_eq!(statuses(&home), Vec::<&str>::new());
    assert_eq!(
        home.problems().first().map(String::as_str),
        Some("docs/agents/flow.toml: not found")
    );
}

#[test]
fn a_home_with_a_file_written_reads_it_in_place_of_the_old_one() {
    let home = Snapshot::of(inputs(Ok(FLOW)));
    let moved = home.with(task("ready"));
    assert_eq!(moved.files().len(), 1);
    assert_eq!(statuses(&moved), ["ready"]);
    assert_eq!(statuses(&home), ["open"]);
}
