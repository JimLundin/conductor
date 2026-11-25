use vcs::status::{Checkout, Distance, Upstream};

#[test]
fn a_checkout_s_state_reads_its_commit_branch_upstream_distance_and_changes() {
    assert_eq!(
        super::status(
            "# branch.oid 1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b\n\
             # branch.head main\n\
             # branch.upstream origin/main\n\
             # branch.ab +2 -3\n\
             ? b.rs\n",
        ),
        Checkout {
            commit: Some(String::from(
                "1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b"
            )),
            branch: Some(String::from("main")),
            upstream: Some(Upstream {
                name: String::from("origin/main"),
                distance: Some(Distance {
                    ahead: 2,
                    behind: 3
                }),
            }),
            changed: 1,
        }
    );
}

#[test]
fn a_detached_checkout_without_commits_or_upstream_has_none_of_them() {
    let state =
        super::status("# branch.oid (initial)\n# branch.head (detached)\n");
    assert_eq!(state, Checkout::default());
}

#[test]
fn only_a_clean_checkout_on_the_default_branch_level_with_origin_is_level() {
    let level = "# branch.oid 1a2b3c4d\n# branch.head main\n\
                 # branch.upstream origin/main\n# branch.ab +0 -0\n";
    assert!(super::status(level).is_level("main"));
    for off in [
        "# branch.oid 1a2b3c4d\n# branch.head main\n\
         # branch.upstream origin/main\n# branch.ab +0 -1\n",
        "# branch.oid 1a2b3c4d\n# branch.head main\n\
         # branch.upstream origin/main\n",
        "# branch.oid 1a2b3c4d\n# branch.head work\n\
         # branch.upstream origin/work\n# branch.ab +0 -0\n",
        "# branch.oid 1a2b3c4d\n# branch.head main\n\
         # branch.upstream fork/main\n# branch.ab +0 -0\n",
        "# branch.oid 1a2b3c4d\n# branch.head main\n\
         # branch.upstream origin/main\n# branch.ab +0 -0\n? new.py\n",
    ] {
        assert!(!super::status(off).is_level("main"), "{off}");
    }
    assert!(!super::status(level).is_level("trunk"));
}

#[test]
fn a_checkout_s_state_reads_as_git_s_short_status_branch_line() {
    let said = |porcelain: &str| super::status(porcelain).to_string();
    assert_eq!(
        said(
            "# branch.oid 1a\n# branch.head main\n\
              # branch.upstream origin/main\n# branch.ab +1 -2\n"
        ),
        "main...origin/main [ahead 1, behind 2]"
    );
    assert_eq!(
        said(
            "# branch.oid 1a\n# branch.head main\n\
              # branch.upstream origin/main\n"
        ),
        "main...origin/main [gone]"
    );
    assert_eq!(said("# branch.oid 1a\n# branch.head main\n"), "main");
    assert_eq!(
        said("# branch.oid 1a\n# branch.head (detached)\n"),
        "HEAD (no branch)"
    );
}

#[test]
fn origin_s_default_branch_reads_from_its_symbolic_ref() {
    assert_eq!(
        super::origin_default("origin/main\n").as_deref(),
        Some("main")
    );
    assert_eq!(super::origin_default("upstream/main\n"), None);
}

fn probe(
    path: &str,
    roots: &[&str],
    remote: Option<&str>,
) -> vcs::discovery::Probe {
    vcs::discovery::Probe {
        path: String::from(path),
        roots: roots.iter().map(|root| String::from(*root)).collect(),
        remote: remote.map(String::from),
    }
}

#[test]
fn only_the_top_of_a_repository_with_a_commit_is_probed() {
    let path = "/home/me/conductor";
    assert_eq!(
        [
            super::probed(
                path,
                Some("/home/me/conductor\n"),
                Some("a\nb\n"),
                Some(" url\n")
            ),
            super::probed(path, Some(path), Some("a"), Some("")),
            super::probed(path, Some(path), Some("a"), None),
            super::probed(path, Some("/home/me"), Some("a"), None),
            super::probed(path, None, Some("a"), None),
            super::probed(path, Some(path), None, None),
            super::probed(path, Some(path), Some(""), None),
        ],
        [
            Some(probe(path, &["a", "b"], Some("url"))),
            Some(probe(path, &["a"], None)),
            Some(probe(path, &["a"], None)),
            None,
            None,
            None,
            None,
        ]
    );
}

#[test]
fn recorded_snapshots_require_full_object_ids() {
    assert!(super::full_commit(
        "1111111111111111111111111111111111111111"
    ));
    assert!(!super::full_commit("HEAD"));
    assert!(!super::full_commit(
        "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"
    ));
}

#[test]
fn output_one_byte_past_the_patch_limit_is_refused_as_oversized() {
    let captured = super::Output {
        bytes: vec![b'a'; super::Output::CAPTURE_TO_SEE_AN_OVERSIZED_PATCH],
        success: true,
        error: String::new(),
    };
    assert_eq!(
        super::checked(captured, "git diff").err(),
        Some(vcs::Error::Failed(String::from(
            "git diff: Git output exceeded the capture limit"
        )))
    );
    assert_eq!(super::Output::CAPTURE_TO_SEE_AN_OVERSIZED_PATCH, 65537);
}
