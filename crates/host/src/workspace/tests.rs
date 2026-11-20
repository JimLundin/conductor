use proto::{
    Change, CheckpointFile, CheckpointId, DiffExtent, FilePatch, HostPath,
    Kept, SessionId, Workspace,
};

use super::{
    Entry, Facts, GitError, Output, PATCH_LIMIT, Step, branch, budget, checked,
    checkpoints, existing, host_path, kept, listing, owned, owner, patch, plan,
    probed, reference, reuse, subtree, text, worktree,
};

fn output(bytes: &[u8]) -> Output {
    Output {
        bytes: Vec::from(bytes),
        success: true,
        error: String::new(),
    }
}

fn failed(error: &str) -> Output {
    Output {
        bytes: Vec::new(),
        success: false,
        error: String::from(error),
    }
}

fn failure(text: &str) -> Result<(), GitError> {
    Err(GitError::Failed(String::from(text)))
}

fn too_much() -> Vec<u8> {
    "a".repeat(PATCH_LIMIT + 1).into_bytes()
}

const SESSION: SessionId = SessionId(1);

fn workspace() -> Workspace {
    let path = |text: &str| HostPath(String::from(text));
    Workspace {
        repository: path("/repo"),
        original: path("/repo/sub"),
        worktree: path("/data/worktrees/00000000000000000000000001"),
        directory: path("/data/worktrees/00000000000000000000000001/sub"),
        branch: branch(SESSION),
    }
}

fn entry(path: &str, change: Change) -> Entry {
    Entry {
        path: String::from(path),
        change,
    }
}

fn file(name: &str, patch: FilePatch) -> CheckpointFile {
    CheckpointFile {
        path: String::from(name),
        change: Change::Edit,
        patch,
    }
}

#[test]
fn a_sessions_worktree_and_branch_are_named_by_its_ulid() {
    assert_eq!(worktree(SESSION), "00000000000000000000000001");
    assert_eq!(branch(SESSION), "conductor/00000000000000000000000001");
    assert_eq!(owner(&workspace()), Ok(worktree(SESSION)));
    assert_eq!(
        reference(&workspace(), CheckpointId(4)),
        Ok(String::from("refs/conductor/00000000000000000000000001/4"))
    );
    assert_eq!(
        checkpoints(&workspace()),
        Ok(String::from("refs/conductor/00000000000000000000000001/"))
    );
}

#[test]
fn a_branch_conductor_does_not_own_is_refused() {
    let mut foreign = workspace();
    foreign.branch = String::from("main");
    assert_eq!(
        owner(&foreign).map(drop),
        failure("workspace branch: not owned by Conductor")
    );
    assert_eq!(
        reference(&foreign, CheckpointId(1)).map(drop),
        failure("checkpoint: not an owned branch")
    );
    foreign.branch = String::from("conductor/not-a-ulid");
    assert!(owner(&foreign).is_err());
}

#[test]
fn paths_must_be_utf8_without_newlines() {
    assert_eq!(host_path(Some("/a b")), Ok(HostPath(String::from("/a b"))));
    assert_eq!(
        host_path(Some("/a\nb")).map(drop),
        failure("Git workspace path: newline paths are unsupported")
    );
    assert_eq!(
        host_path(None).map(drop),
        failure("Git workspace path: non-UTF-8 paths are unsupported")
    );
}

#[test]
fn git_output_is_checked_for_success_and_size() {
    assert_eq!(text(output(b"/repo\n\n"), "top"), Ok(String::from("/repo")));
    assert_eq!(
        checked(failed("fatal: no"), "top").map(drop),
        failure("top: fatal: no")
    );
    let oversized = too_much();
    assert_eq!(
        checked(output(&oversized), "top").map(drop),
        failure("top: Git output exceeded the capture limit")
    );
    assert_eq!(
        text(output(&[255]), "top").map(drop),
        Err(GitError::Failed(String::from(
            "Git path or metadata is not UTF-8: invalid utf-8 sequence of 1 bytes from index 0"
        )))
    );
}

#[test]
fn a_failed_probe_means_no_committed_repository() {
    assert_eq!(
        probed(failed("fatal"), "root"),
        Err(GitError::NotRepository)
    );
    assert_eq!(probed(output(b"abc\n"), "root"), Ok(String::from("abc\n")));
}

#[test]
fn only_a_subdirectory_is_checked_for_a_tree() {
    assert_eq!(subtree("abc", &HostPath(String::new())), None);
    assert_eq!(
        subtree("abc", &HostPath(String::from("sub dir"))),
        Some(String::from("abc:sub dir"))
    );
}

#[test]
fn a_commit_is_owned_when_a_checkpoint_ref_names_it() {
    assert_eq!(owned("abc\ndef", "def"), Ok(()));
    assert_eq!(owned("def", "def"), Ok(()));
    assert_eq!(
        owned("abc", "def"),
        failure(
            "checkpoint: commit is not an immutable checkpoint of this workspace"
        )
    );
    assert_eq!(
        owned("", "def"),
        failure(
            "checkpoint: commit is not an immutable checkpoint of this workspace"
        )
    );
}

#[test]
fn a_checkpoint_id_taken_again_must_record_the_same_tree() {
    assert_eq!(existing(failed("")), Ok(None));
    assert_eq!(existing(output(b"abc\n")), Ok(Some(String::from("abc"))));
    assert_eq!(reuse("t", "t"), Ok(()));
    assert_eq!(
        reuse("t", "u"),
        failure(
            "checkpoint id already records a different tree: metadata differs"
        )
    );
}

#[test]
fn a_turn_that_changed_no_file_makes_no_commit() {
    assert_eq!(super::turn("t", "t"), super::Turn::Tip);
    assert_eq!(super::turn("t", "u"), super::Turn::Commit);
}

#[test]
fn the_name_status_listing_is_parsed() {
    assert_eq!(listing(output(b"")), Ok((Vec::new(), DiffExtent::Complete)));
    assert_eq!(
        listing(output(b"A\0new \xe9\x9b\xaa.txt\0D\0gone\0M\0kept\0")),
        Ok((
            Vec::from([
                entry("new 雪.txt", Change::Write),
                entry("gone", Change::Delete),
                entry("kept", Change::Edit),
            ]),
            DiffExtent::Complete
        ))
    );
    assert_eq!(
        listing(output(b"A\0")).map(drop),
        failure("diff metadata: missing path")
    );
    assert_eq!(
        listing(output(&too_much())),
        Ok((Vec::new(), DiffExtent::Truncated))
    );
}

#[test]
fn a_listing_past_the_file_limit_is_truncated() {
    let many = (0..=100).fold(Vec::new(), |mut bytes, number| {
        bytes.extend_from_slice(format!("M\0file-{number:03}\0").as_bytes());
        bytes
    });
    assert_eq!(
        listing(output(&many)).map(|(entries, extent)| (entries.len(), extent)),
        Ok((100, DiffExtent::Truncated))
    );
}

#[test]
fn a_patch_is_text_binary_too_large_or_unsupported() {
    let edit = || entry("f", Change::Edit);
    let classified =
        |bytes: &[u8]| patch(output(bytes), edit()).map(|file| file.patch);
    assert_eq!(
        classified(b"+a\n"),
        Ok(FilePatch::Text(String::from("+a\n")))
    );
    assert_eq!(
        classified(b"diff\nBinary files a and b differ\n"),
        Ok(FilePatch::Binary)
    );
    assert_eq!(classified(&too_much()), Ok(FilePatch::TooLarge));
    assert_eq!(
        classified(&[255]),
        Ok(FilePatch::Unsupported {
            reason: String::from(
                "patch is not UTF-8: invalid utf-8 sequence of 1 bytes from index 0"
            ),
        })
    );
}

#[test]
fn patches_past_the_total_budget_are_sent_as_too_large() {
    let big = || FilePatch::Text("a".repeat(PATCH_LIMIT));
    let files = Vec::from([
        file("1", big()),
        file("2", big()),
        file("3", big()),
        file("4", big()),
        file("5", big()),
        file("6", FilePatch::Binary),
    ]);
    let (kept, extent) = budget(files, DiffExtent::Complete);
    assert_eq!(extent, DiffExtent::Truncated);
    assert_eq!(
        kept.iter()
            .map(|file| file.patch.clone())
            .collect::<Vec<_>>(),
        [
            big(),
            big(),
            big(),
            big(),
            FilePatch::TooLarge,
            FilePatch::Binary
        ]
    );
    assert_eq!(
        budget(Vec::from([file("1", big())]), DiffExtent::Complete).1,
        DiffExtent::Complete
    );
}

fn facts(tree: Option<&str>, origin: Option<&str>) -> Facts {
    Facts {
        head_tree: String::from("head"),
        tree: tree.map(String::from),
        origin: origin.map(String::from),
    }
}

fn push() -> Step {
    let name = branch(SESSION);
    Step::Push {
        remote: String::from("file:///remote.git"),
        args: Vec::from([
            String::from("push"),
            String::from("--quiet"),
            String::from("origin"),
            format!("refs/heads/{name}:refs/heads/{name}"),
        ]),
    }
}

fn forget() -> Step {
    Step::Forget(String::from("refs/conductor/00000000000000000000000001/"))
}

#[test]
fn a_dirty_workspace_is_committed_pushed_then_removed_with_its_refs() {
    assert_eq!(
        plan(
            &workspace(),
            &facts(Some("dirty"), Some("file:///remote.git"))
        ),
        Ok(Vec::from([
            Step::Commit {
                tree: String::from("dirty")
            },
            push(),
            Step::Remove,
            forget(),
        ]))
    );
}

#[test]
fn a_clean_workspace_without_a_remote_keeps_its_branch_locally() {
    assert_eq!(
        plan(&workspace(), &facts(Some("head"), None)),
        Ok(Vec::from([Step::Remove, forget()]))
    );
    assert_eq!(kept(None), Kept::Local);
}

#[test]
fn a_workspace_whose_directory_is_gone_is_pruned() {
    assert_eq!(
        plan(&workspace(), &facts(None, Some("file:///remote.git"))),
        Ok(Vec::from([push(), Step::Prune, forget()]))
    );
}

#[test]
fn a_failed_push_keeps_the_branch_locally_and_a_pushed_one_names_the_remote() {
    assert_eq!(
        kept(Some((
            String::from("file:///remote.git"),
            Err(String::from("rejected"))
        ))),
        Kept::PushFailed {
            reason: String::from("rejected")
        }
    );
    assert_eq!(
        kept(Some((String::from("file:///remote.git"), Ok(())))),
        Kept::Pushed {
            remote: String::from("file:///remote.git")
        }
    );
}

#[test]
fn only_a_branch_of_conductors_own_is_ever_pushed_and_never_forced() {
    let foreign = Workspace {
        branch: String::from("main"),
        ..workspace()
    };
    assert_eq!(
        plan(&foreign, &facts(Some("dirty"), Some("origin"))),
        Err(GitError::Failed(String::from(
            "workspace branch: not owned by Conductor"
        )))
    );
    let pushes = plan(&workspace(), &facts(Some("dirty"), Some("origin")))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|step| match step {
            Step::Push { args, .. } => Some(args),
            _ => None,
        })
        .collect::<Vec<Vec<String>>>();
    assert_eq!(pushes.len(), 1);
    assert!(
        pushes.iter().flatten().all(|arg| !arg.starts_with('+')
            && arg != "--force"
            && arg != "-f")
    );
}

#[test]
fn a_checkpoint_moves_the_branch_on_from_its_parent_and_never_drops_commits() {
    assert_eq!(super::settle("c", "c", Some("p")), Ok(false));
    assert_eq!(super::settle("p", "c", Some("p")), Ok(true));
    assert_eq!(
        super::settle("x", "c", Some("p")),
        Err(GitError::Failed(String::from(
            "checkpoint: the session's branch moved on since the checkpoint was taken"
        )))
    );
}

#[test]
fn a_listing_of_exactly_the_file_limit_is_complete() {
    let all = (0..super::FILE_LIMIT).fold(Vec::new(), |mut bytes, number| {
        bytes.extend_from_slice(format!("M\0file-{number:03}\0").as_bytes());
        bytes
    });
    assert_eq!(
        listing(output(&all)).map(|(entries, extent)| (entries.len(), extent)),
        Ok((super::FILE_LIMIT, DiffExtent::Complete))
    );
}

#[test]
fn output_is_oversized_only_past_the_patch_limit() {
    assert!(!super::oversized(PATCH_LIMIT));
    assert!(super::oversized(PATCH_LIMIT + 1));
}

#[test]
fn the_marker_records_the_workspace_one_field_per_line() {
    let workspace = workspace();
    assert_eq!(
        super::marker(&workspace),
        format!(
            "/repo\n/repo/sub\n/data/worktrees/00000000000000000000000001\n\
             /data/worktrees/00000000000000000000000001/sub\n{}\n",
            workspace.branch
        )
    );
}

#[test]
fn a_turns_commit_names_its_checkpoint() {
    assert_eq!(
        super::message(CheckpointId(3)),
        "Conductor: checkpoint 3 of the session"
    );
}
