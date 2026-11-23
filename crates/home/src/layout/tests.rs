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

#[test]
fn an_issue_path_splits_its_directories_from_its_file() {
    let path = super::IssuePath::checked(Some(Vec::from([
        "projects", "a1b2c3", "issues", "a.md",
    ])));
    assert!(
        path.is_ok_and(|split| split.parents
            == ["projects", "a1b2c3", "issues"]
            && split.leaf == "a.md")
    );
}

#[test]
fn an_issue_path_with_a_component_that_isn_t_a_name_or_no_file_is_unsafe() {
    assert!(super::IssuePath::<&str>::checked(None).is_err());
    assert!(super::IssuePath::<&str>::checked(Some(Vec::new())).is_err());
}

#[test]
fn only_a_write_creates_a_missing_issue_directory() {
    use super::{IssueParent, issue_parent};
    assert!(matches!(issue_parent(true, false), IssueParent::Opened));
    assert!(matches!(issue_parent(true, true), IssueParent::Opened));
    assert!(matches!(issue_parent(false, false), IssueParent::Absent));
    assert!(matches!(issue_parent(false, true), IssueParent::Create));
}

#[test]
fn an_issue_file_is_missing_or_an_ordinary_file_never_a_link() {
    use super::{IssueLeaf, UnsafeIssueEntry, issue_leaf};
    let leaf = |regular, reparse_point| IssueLeaf {
        regular,
        reparse_point,
    };
    assert_eq!(issue_leaf(None), Ok(()));
    assert_eq!(issue_leaf(Some(leaf(true, false))), Ok(()));
    assert_eq!(issue_leaf(Some(leaf(false, false))), Err(UnsafeIssueEntry));
    assert_eq!(issue_leaf(Some(leaf(true, true))), Err(UnsafeIssueEntry));
}
