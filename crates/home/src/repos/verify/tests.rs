use std::collections::BTreeMap;

use super::{
    Ran, Reason, Verify, Verifying, asks, origin_default, verdict, verifying,
};
use crate::ProjectId;
use crate::checkout::Probe;
use crate::project::Project;
use crate::repos::{Checkout, Placed, placed};

const STANDARDS: &str = "5d923acab7b653f45219ab8ee5feeb5b5ddd9318";
const APP: &str = "6094e296778e04d108490df49fa9a20d4b382c60";
const LEVEL: &str = "# branch.oid 1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b\n\
                     # branch.head main\n\
                     # branch.upstream origin/main\n\
                     # branch.ab +0 -0\n";
const BEHIND: &str = "# branch.oid 1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b\n\
                      # branch.head main\n\
                      # branch.upstream origin/main\n\
                      # branch.ab +0 -1\n";

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
/// each checked out at its name, and the standards also at `checkouts`.
fn home(profiles: &[&str], checkouts: &[&str]) -> Vec<Placed> {
    let mut standards = project("wcyv7j", "standards", STANDARDS);
    standards.provides = strings(&["python", "rust"]);
    let mut app = project("2rr5p0", "app", APP);
    app.profiles = strings(profiles);
    let probe = |path: &str, root: &str| Probe {
        path: String::from(path),
        roots: Vec::from([String::from(root)]),
        remote: None,
    };
    let probes: Vec<Probe> = checkouts
        .iter()
        .map(|path| probe(path, STANDARDS))
        .chain([probe("app", APP)])
        .collect();
    placed(&[standards, app], &probes)
}

fn states(of: &[(&str, &str)]) -> BTreeMap<String, Option<Checkout>> {
    of.iter()
        .map(|(path, porcelain)| {
            (String::from(*path), Some(Checkout::parse(porcelain)))
        })
        .collect()
}

fn defaults(paths: &[&str]) -> BTreeMap<String, String> {
    paths
        .iter()
        .map(|path| (String::from(*path), String::from("main")))
        .collect()
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

fn said(verifies: &[Verifying]) -> Vec<String> {
    verifies
        .iter()
        .map(|verifying| match verifying {
            Verifying::Run(verify) => format!("run {verify:?}"),
            Verifying::Unchecked(reason) => reason.to_string(),
        })
        .collect()
}

#[test]
fn each_profile_of_a_project_level_at_its_default_head_is_verified_from_the_standards_checkout()
 {
    assert_eq!(
        verifying(
            "/h",
            &home(&["python", "rust"], &["standards"]),
            &states(&[("standards", LEVEL), ("app", LEVEL)]),
            &defaults(&["standards", "app"]),
        ),
        [
            Verifying::Run(app_verify("python")),
            Verifying::Run(app_verify("rust")),
        ]
    );
}

#[test]
fn a_project_whose_checkout_is_off_its_default_head_or_changed_is_unchecked() {
    for state in [
        BEHIND,
        "# branch.oid 1a2b\n# branch.head work\n\
         # branch.upstream origin/work\n# branch.ab +0 -0\n",
        "# branch.oid 1a2b\n# branch.head main\n\
         # branch.upstream origin/main\n# branch.ab +0 -0\n? new.py\n",
    ] {
        assert_eq!(
            said(&verifying(
                "/h",
                &home(&["python"], &["standards"]),
                &states(&[("standards", LEVEL), ("app", state)]),
                &defaults(&["standards", "app"]),
            )),
            ["app: no checkout clean at origin/main, so its recorded \
              passes go unchecked\n"]
        );
    }
}

#[test]
fn a_behind_or_dirty_standards_checkout_gives_only_its_own_line() {
    for state in [
        BEHIND,
        "# branch.oid 1a2b\n# branch.head main\n\
         # branch.upstream origin/main\n# branch.ab +0 -0\n\
         1 .M N... 100644 100644 100644 1a2b 1a2b lib.sh\n",
    ] {
        assert_eq!(
            said(&verifying(
                "/h",
                &home(&["python", "rust"], &["standards"]),
                &states(&[("standards", state), ("app", BEHIND)]),
                &defaults(&["standards", "app"]),
            )),
            [
                "standards at /h/standards isn't clean at origin/main; fix: \
              bring it level with origin\n"
            ]
        );
    }
}

#[test]
fn a_level_standards_checkout_is_used_when_the_first_one_is_not() {
    assert_eq!(
        verifying(
            "/h",
            &home(&["python"], &["standards-work", "standards"]),
            &states(&[
                ("standards-work", BEHIND),
                ("standards", LEVEL),
                ("app", LEVEL),
            ]),
            &defaults(&["standards-work", "standards", "app"]),
        ),
        [Verifying::Run(app_verify("python"))]
    );
}

#[test]
fn a_profile_nobody_provides_is_told_apart_from_a_provider_not_checked_out() {
    assert_eq!(
        said(&verifying(
            "/h",
            &home(&["cobol"], &["standards"]),
            &states(&[("standards", LEVEL), ("app", LEVEL)]),
            &defaults(&["standards", "app"]),
        )),
        ["app: nobody provides cobol, so its recorded passes go unchecked\n"]
    );
    assert_eq!(
        said(&verifying(
            "/h",
            &home(&["python"], &[]),
            &states(&[("app", LEVEL)]),
            &defaults(&["app"]),
        )),
        [
            "app: standards isn't checked out here, so its python passes go \
          unchecked\n"
        ]
    );
}

#[test]
fn a_project_without_profiles_is_not_verified() {
    let verifies = verifying(
        "/h",
        &home(&[], &["standards"]),
        &states(&[("standards", LEVEL), ("app", LEVEL)]),
        &defaults(&["standards", "app"]),
    );
    assert_eq!(verifies, []);
}

#[test]
fn only_checkouts_of_projects_with_or_providing_profiles_are_asked_their_default()
 {
    assert_eq!(
        asks(&home(&["python"], &["standards"])),
        ["app", "standards"]
    );
    assert_eq!(asks(&home(&[], &[])), [] as [&str; 0]);
}

#[test]
fn origin_s_default_branch_reads_from_its_symbolic_ref() {
    assert_eq!(origin_default("origin/main\n").as_deref(), Some("main"));
    assert_eq!(origin_default("upstream/main\n"), None);
}

fn ran(passed: bool, out: &str, err: &str) -> Ran {
    Ran {
        passed,
        out: String::from(out),
        err: String::from(err),
    }
}

#[test]
fn a_head_with_every_commit_unit_recorded_shows_no_report() {
    let passed =
        ran(true, "format               ok    passed (recorded)\n", "");
    assert_eq!(verdict(&app_verify("python"), &passed), None);
}

#[test]
fn no_recorded_pass_lines_name_their_units_once_and_the_command_to_run() {
    let missing = ran(
        false,
        "format               ok    passed (recorded)\n\
         purity               FAIL  no recorded pass of . under 3665\n\
         unused               FAIL  no recorded pass of . under 69ff\n\
         unused               FAIL  no recorded pass of web under 70aa\n\
         tier: commit, logs: /tmp/x\n",
        "",
    );
    assert_eq!(
        verdict(&app_verify("python"), &missing)
            .map(|reason| reason.to_string()),
        Some(String::from(
            "app: no recorded `commit` pass for python at 1a2b3c4 (purity, \
             unused); fix: run `python/check.sh commit /h/app` from \
             /h/standards\n"
        ))
    );
}

#[test]
fn a_config_failure_reads_check_sh_failed_with_its_first_stderr_line() {
    let failed = ran(
        false,
        "suites               FAIL  checks.toml: names no pinned tool OXFMT\n\
         lint                 FAIL  no recorded pass of . under 5145\n",
        "\nchecks.toml: names no pinned tool OXFMT\nmore\n",
    );
    assert_eq!(
        verdict(&app_verify("rust"), &failed).map(|reason| reason.to_string()),
        Some(String::from(
            "app: rust/check.sh failed: checks.toml: names no pinned tool \
             OXFMT\n"
        ))
    );
}

#[test]
fn a_failure_without_stderr_reads_check_sh_failed_with_its_failing_line() {
    let failed = ran(
        false,
        "suites               FAIL  checks.toml: suite 4 when 1\n",
        "",
    );
    assert_eq!(
        verdict(&app_verify("rust"), &failed),
        Some(Reason::CheckFailed {
            project: String::from("app"),
            profile: String::from("rust"),
            why: String::from(
                "suites               FAIL  checks.toml: suite 4 when 1"
            ),
        })
    );
}
