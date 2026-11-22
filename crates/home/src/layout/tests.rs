use proto::ProjectId;

use super::{IssueLocation, inputs, issue_files, issue_stem, project_file};
use crate::flow::graph::File;

fn alpha() -> ProjectId {
    ProjectId::parse("a1b2c3").unwrap_or(ProjectId::new(0))
}

#[test]
fn a_project_is_described_by_the_project_file_in_its_folder() {
    assert_eq!(project_file("a1b2c3"), "projects/a1b2c3/project.toml");
}

#[test]
fn only_md_files_of_a_project_s_issues_folder_are_issues_by_name() {
    let names = ["b.md", "notes.txt", "a.md", ".md.swp"]
        .map(String::from)
        .to_vec();
    assert_eq!(
        issue_files(&alpha(), names),
        [
            IssueLocation {
                project: alpha(),
                stem: String::from("a"),
                path: String::from("projects/a1b2c3/issues/a.md"),
            },
            IssueLocation {
                project: alpha(),
                stem: String::from("b"),
                path: String::from("projects/a1b2c3/issues/b.md"),
            },
        ]
    );
}

#[test]
fn an_issue_s_stem_is_its_file_s_name_without_md() {
    assert_eq!(issue_stem("projects/a1b2c3/issues/a.md"), Some("a"));
    assert_eq!(issue_stem("projects/a1b2c3/project.toml"), None);
}

#[test]
fn an_issue_file_that_couldn_t_be_read_is_unread_by_its_path() {
    let [read, failed] = ["a", "b"].map(|stem| IssueLocation {
        project: alpha(),
        stem: String::from(stem),
        path: format!("projects/a1b2c3/issues/{stem}.md"),
    });
    let made = inputs(
        Ok(String::from("flow")),
        Vec::new(),
        Vec::from([
            (read, Ok(String::from("text"))),
            (failed, Err(String::from("denied"))),
        ]),
    );
    assert_eq!(
        made.issues,
        [File {
            project: alpha(),
            stem: String::from("a"),
            text: String::from("text"),
        }]
    );
    assert_eq!(made.unread, ["projects/a1b2c3/issues/b.md: denied"]);
}
