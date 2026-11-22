use std::collections::BTreeMap;

use proto::ProjectId;

use super::{
    Asked, Fetch, Scanning, asked, placed, probe, profiles, regenerated,
    report, repositories, scan, sync, table, unmatched, warnings,
};
use crate::checkout::Probe;
use crate::project::{Project, parse};

const ROOT: &str = "fc27e50efb661d7ef4a45d3edbe0234ff0a6fbd5";
const OTHER: &str = "d2cef7849f3ce7cc16f17979e0acb8c21d9aed2a";

fn project(id: &str, name: &str, root: &str, remote: Option<&str>) -> Project {
    Project {
        id: ProjectId::parse(id).unwrap_or(ProjectId::new(0)),
        name: String::from(name),
        title: String::from(name),
        remote: remote.map(String::from),
        root: String::from(root),
        profiles: Vec::new(),
        provides: Vec::new(),
    }
}

fn repository(path: &str, root: &str, remote: Option<&str>) -> Probe {
    Probe {
        path: String::from(path),
        roots: Vec::from([String::from(root)]),
        remote: remote.map(String::from),
    }
}

fn words(line: &str) -> Vec<String> {
    line.split_whitespace().map(String::from).collect()
}

#[test]
fn the_words_ask_for_repos_sync_scan_or_skills() {
    assert_eq!(asked(&words("repos")), Some(Asked::Status));
    assert_eq!(asked(&words("repos sync")), Some(Asked::Sync));
    assert_eq!(asked(&words("repos scan")), Some(Asked::Scan));
    assert_eq!(asked(&words("skills")), Some(Asked::Skills));
    assert_eq!(asked(&words("repos wipe")), None);
    assert_eq!(asked(&words("check")), None);
}

#[test]
fn the_home_comes_first_then_sorted_repositories_but_none_set_aside() {
    let found = ["apps/conductor", "_old/thing", "keyframe", "Archives/x"]
        .map(String::from)
        .to_vec();
    assert_eq!(repositories(found), [".", "apps/conductor", "keyframe"]);
}

#[test]
fn a_repository_without_a_commit_is_no_probe() {
    assert_eq!(probe("empty", None, None), None);
    assert_eq!(probe("empty", Some(""), None), None);
    assert_eq!(
        probe("sop", Some(&format!("{ROOT}\n")), Some("\n")),
        Some(repository("sop", ROOT, None))
    );
}

#[test]
fn a_checkout_has_the_project_s_root_and_its_remote_in_any_form() {
    let projects = [
        project(
            "cnd000",
            "conductor",
            ROOT,
            Some("https://github.com/JimLundin/conductor.git"),
        ),
        project("s0p000", "sop", OTHER, None),
    ];
    let probes = [
        repository(
            "apps/conductor",
            ROOT,
            Some("git@GitHub.com:JimLundin/conductor"),
        ),
        repository("fork", ROOT, Some("https://github.com/someone/conductor")),
        repository("sop", OTHER, None),
        repository("unrelated", OTHER, Some("https://example.com/x.git")),
    ];
    let found = placed(&projects, &probes);
    let paths: Vec<(&str, Vec<&str>)> = found
        .iter()
        .map(|placed| {
            (
                placed.name.as_str(),
                placed.checkouts.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    assert_eq!(
        paths,
        [
            ("conductor", Vec::from(["apps/conductor"])),
            ("sop", Vec::from(["sop"])),
        ]
    );
    let left: Vec<String> = unmatched(&projects, &probes)
        .into_iter()
        .map(|probe| probe.path)
        .collect();
    assert_eq!(left, ["fork", "unrelated"]);
}

#[test]
fn the_report_gives_each_checkout_s_branch_and_changes_by_project_name() {
    let projects = [
        project("zeta00", "zeta", OTHER, None),
        project("a1fa00", "alpha", ROOT, None),
    ];
    let found = placed(&projects, &[repository("alpha", ROOT, None)]);
    let states = BTreeMap::from([(
        String::from("alpha"),
        Some(String::from(
            "## main...origin/main [ahead 1]\n M a.rs\n?? b.rs\n",
        )),
    )]);
    assert_eq!(
        report(&found, &states),
        "alpha                  alpha: main...origin/main [ahead 1]  [2 changed]\n\
         zeta                   not checked out here\n"
    );
}

#[test]
fn sync_clones_each_project_with_a_remote_and_no_checkout_into_its_name() {
    let projects = [
        project("a00000", "a", ROOT, Some("https://example.com/a.git")),
        project("b00000", "b", OTHER, Some("https://example.com/b.git")),
        project("s0p000", "sop", OTHER, None),
    ];
    let found = placed(
        &projects,
        &[repository("a", ROOT, Some("https://example.com/a"))],
    );
    assert_eq!(
        sync(&found),
        [
            Fetch {
                said: String::from("clone https://example.com/b.git into b\n"),
                clone: Some((
                    String::from("https://example.com/b.git"),
                    String::from("b")
                )),
            },
            Fetch {
                said: String::from("skip sop: local only, no remote\n"),
                clone: None,
            },
        ]
    );
}

#[test]
fn a_repository_s_profiles_follow_the_files_at_its_top() {
    let names = ["Cargo.toml", "README.md", "package.json"]
        .map(String::from)
        .to_vec();
    assert_eq!(profiles(&names), ["rust", "typescript"]);
    assert_eq!(profiles(&[String::from("pyproject.toml")]), ["python"]);
    assert_eq!(profiles(&[]), Vec::<String>::new());
}

#[test]
fn the_table_lists_each_project_by_name_with_its_remote() {
    let projects = [
        project("s0p000", "sop", OTHER, None),
        project("a00000", "home", ROOT, Some("https://example.com/home.git")),
    ];
    assert_eq!(
        table(&projects),
        "<!-- repos:start (generated by home repos scan from projects/) -->\n\
         | Project | Remote |\n\
         |---|---|\n\
         | `home` | https://example.com/home.git |\n\
         | `sop` | local only |\n\
         <!-- repos:end -->\n"
    );
}

#[test]
fn the_table_replaces_the_lines_between_its_markers_only() {
    let doc = "# Home\n\n<!-- repos:start (old) -->\n| old |\n<!-- repos:end -->\nafter\n";
    assert_eq!(
        regenerated(doc, "<!-- repos:start -->\nnew\n<!-- repos:end -->\n"),
        "# Home\n\n<!-- repos:start -->\nnew\n<!-- repos:end -->\nafter\n"
    );
    assert_eq!(regenerated("no table\n", "table\n"), "no table\n");
}

fn scanning(new: Vec<(Probe, Vec<String>)>, draws: Vec<u32>) -> Scanning {
    let home = project("a00000", "home", ROOT, None);
    Scanning {
        home: String::from("projects"),
        folders: Vec::from([String::from("a00000")]),
        projects: Vec::from([home]),
        new,
        draws,
        problems: Vec::new(),
        doc: Ok(String::from(
            "intro\n<!-- repos:start -->\n<!-- repos:end -->\n",
        )),
    }
}

#[test]
fn scan_registers_a_new_repository_named_after_its_folder() -> Result<(), String>
{
    let new = Vec::from([(
        repository("apps/My App", OTHER, Some("https://example.com/app.git")),
        Vec::from([String::from("Cargo.toml")]),
    )]);
    let done = scan(scanning(new, Vec::from([5])))?;
    assert_eq!(
        done.writes,
        [
            (
                String::from("projects/000005/project.toml"),
                String::from(
                    "name = \"my-app\"\ntitle = \"My App\"\n\
                     remote = \"https://example.com/app.git\"\n\
                     root = \"d2cef7849f3ce7cc16f17979e0acb8c21d9aed2a\"\n\
                     profiles = [\"rust\"]\n"
                )
            ),
            (
                String::from("AGENTS.md"),
                String::from(
                    "intro\n\
                     <!-- repos:start (generated by home repos scan from projects/) -->\n\
                     | Project | Remote |\n|---|---|\n\
                     | `home` | local only |\n\
                     | `my-app` | https://example.com/app.git |\n\
                     <!-- repos:end -->\n"
                )
            ),
        ]
    );
    assert_eq!(
        done.said,
        "added my-app as projects/000005 from apps/My App\nhome\nmy-app\n"
    );
    Ok(())
}

#[test]
fn scan_draws_again_for_an_id_the_home_holds_or_has_just_minted()
-> Result<(), String> {
    let new = Vec::from([
        (
            repository(".", ROOT, Some("https://example.com/home")),
            Vec::new(),
        ),
        (repository("sop", OTHER, None), Vec::new()),
    ]);
    let first = [0, 3].into_iter().chain(std::iter::repeat(0));
    let second = [0, 3, 7].into_iter().chain(std::iter::repeat(0));
    let draws = first
        .take(super::DRAWS)
        .chain(second.take(super::DRAWS))
        .collect();
    let mut input = scanning(new, draws);
    input.folders = Vec::from([String::from("000000")]);
    let done = scan(input)?;
    let folders: Vec<&str> =
        done.writes.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(
        folders,
        [
            "projects/000003/project.toml",
            "projects/000007/project.toml",
            "AGENTS.md"
        ]
    );
    let titles: Vec<String> = done
        .writes
        .iter()
        .zip(["000003", "000007"])
        .filter_map(|((_, text), id)| parse(id, text).ok())
        .map(|project| project.title)
        .collect();
    assert_eq!(titles, ["projects", "sop"]);
    Ok(())
}

#[test]
fn scan_gives_up_when_every_draw_is_held() {
    let new = Vec::from([(repository("sop", OTHER, None), Vec::new())]);
    let mut input = scanning(new, Vec::from([0, 0]));
    input.folders = Vec::from([String::from("000000")]);
    assert_eq!(
        scan(input),
        Err(String::from("no fresh project id for sop"))
    );
}

#[test]
fn scan_registers_nothing_while_a_project_folder_does_not_read() {
    let new = Vec::from([(repository("sop", OTHER, None), Vec::new())]);
    let mut input = scanning(new, Vec::from([1]));
    input.problems = Vec::from([String::from("projects/x: no project.toml")]);
    assert_eq!(
        scan(input),
        Err(String::from(
            "scan registers nothing while the home's projects don't read:\n\
             projects/x: no project.toml"
        ))
    );
}

#[test]
fn each_listing_problem_is_a_warning_line() {
    let problems = Vec::from([
        String::from("projects/x: bad"),
        String::from("projects/y: bad"),
    ]);
    assert_eq!(
        warnings(&problems),
        "home: projects/x: bad\nhome: projects/y: bad\n"
    );
}
