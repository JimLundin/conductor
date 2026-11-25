use std::collections::BTreeMap;

use super::{Head, Ran, Verify, Verifying, verdict, verifying};
use crate::ProjectId;
use crate::checkout::Probe;
use crate::project::Project;
use crate::repos::placed;

const STANDARDS: &str = "5d923acab7b653f45219ab8ee5feeb5b5ddd9318";
const APP: &str = "6094e296778e04d108490df49fa9a20d4b382c60";
const AT_HEAD: &str = "## main...origin/main\n";

fn project(id: &str, name: &str, root: &str) -> Project {
    Project {
        id: ProjectId::parse(id).unwrap_or(ProjectId::new(0)),
        name: String::from(name),
        title: String::from(name),
        remote: None,
        root: String::from(root),
        profiles: Vec::new(),
        provides: Vec::new(),
    }
}

fn strings(names: &[&str]) -> Vec<String> {
    names.iter().copied().map(String::from).collect()
}

/// The standards, providing python and rust, and `app` with `profiles`,
/// each checked out at its name.
fn home(profiles: &[&str]) -> Vec<crate::repos::Placed> {
    let mut standards = project("wcyv7j", "standards", STANDARDS);
    standards.provides = strings(&["python", "rust"]);
    let mut app = project("2rr5p0", "app", APP);
    app.profiles = strings(profiles);
    let probe = |path: &str, root: &str| Probe {
        path: String::from(path),
        roots: Vec::from([String::from(root)]),
        remote: None,
    };
    placed(
        &[standards, app],
        &[probe("standards", STANDARDS), probe("app", APP)],
    )
}

fn states(app: &str) -> BTreeMap<String, Option<String>> {
    BTreeMap::from([
        (String::from("standards"), Some(String::from(AT_HEAD))),
        (String::from("app"), Some(String::from(app))),
    ])
}

fn heads() -> BTreeMap<String, Head> {
    ["standards", "app"]
        .map(|path| {
            (
                String::from(path),
                Head {
                    commit: Some(String::from("1a2b3c4\n")),
                    default: Some(String::from("origin/main\n")),
                },
            )
        })
        .into()
}

fn app_verify(profile: &str) -> Verify {
    Verify {
        project: String::from("app"),
        profile: String::from(profile),
        commit: String::from("1a2b3c4"),
        checkout: String::from("/h/app"),
        standards: String::from("/h/standards"),
    }
}

#[test]
fn each_profile_of_a_project_clean_at_its_default_head_is_verified_from_the_standards_checkout()
 {
    assert_eq!(
        verifying("/h", &home(&["python", "rust"]), &states(AT_HEAD), &heads()),
        [
            Verifying::Run(app_verify("python")),
            Verifying::Run(app_verify("rust")),
        ]
    );
}

#[test]
fn a_project_whose_checkout_is_off_its_default_head_or_changed_is_skipped() {
    for state in [
        "## main...origin/main [behind 1]\n",
        "## work...origin/work\n",
        "## main...origin/main\n?? new.py\n",
    ] {
        assert_eq!(
            verifying("/h", &home(&["python"]), &states(state), &heads()),
            [Verifying::Said(String::from(
                "app: no checkout clean at origin/main, so its recorded \
                 passes go unchecked\n"
            ))]
        );
    }
}

#[test]
fn a_profile_no_checked_out_project_provides_is_skipped() {
    assert_eq!(
        verifying("/h", &home(&["typescript"]), &states(AT_HEAD), &heads()),
        [Verifying::Said(String::from(
            "app: no project checked out here provides typescript, so its \
             recorded passes go unchecked\n"
        ))]
    );
}

#[test]
fn a_project_without_profiles_is_not_verified() {
    let verifies = verifying("/h", &home(&[]), &states(AT_HEAD), &heads());
    assert_eq!(verifies, []);
}

#[test]
fn a_head_with_every_commit_unit_recorded_shows_no_report() {
    let ran = Ran {
        passed: true,
        out: String::from("format               ok    passed (recorded)\n"),
    };
    assert_eq!(verdict(&app_verify("python"), Some(&ran)), "");
}

#[test]
fn a_head_lacking_recorded_passes_names_its_units_and_the_command_to_run() {
    let ran = Ran {
        passed: false,
        out: String::from(
            "format               ok    passed (recorded)\n\
             purity               FAIL  no recorded pass of . under 3665\n\
             unused               FAIL  no recorded pass of . under 69ff\n\
             tier: commit, logs: /tmp/x\n",
        ),
    };
    assert_eq!(
        verdict(&app_verify("python"), Some(&ran)),
        "app: no recorded `commit` pass for python at 1a2b3c4 (purity, \
         unused); fix: run `python/check.sh commit /h/app` from \
         /h/standards\n"
    );
}

#[test]
fn a_verify_that_fails_without_naming_a_unit_or_does_not_run_says_how_to_see_why()
 {
    let failed = Ran {
        passed: false,
        out: String::new(),
    };
    let said = "app: verifying python at 1a2b3c4 failed without naming a \
                unit; see `python/check.sh commit --verify /h/app` from \
                /h/standards\n";
    assert_eq!(verdict(&app_verify("python"), Some(&failed)), said);
    assert_eq!(verdict(&app_verify("python"), None), said);
}

#[test]
fn a_unit_failing_more_than_once_is_named_once() {
    let ran = Ran {
        passed: false,
        out: String::from(
            "suites               FAIL  checks.toml: suite 4 when 1\n\
             suites               FAIL  checks.toml: suite 5 when 1\n\
             lint                 FAIL  no recorded pass of . under 5145\n",
        ),
    };
    assert_eq!(
        verdict(&app_verify("typescript"), Some(&ran)),
        "app: no recorded `commit` pass for typescript at 1a2b3c4 (suites, \
         lint); fix: run `typescript/check.sh commit /h/app` from \
         /h/standards\n"
    );
}
