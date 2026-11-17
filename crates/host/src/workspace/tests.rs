use proto::{
    Change, CheckpointFile, CheckpointId, DiffExtent, FilePatch, HostPath,
    SessionId, Workspace,
};

use super::{
    Entry, GitError, Output, PATCH_LIMIT, branch, budget, checked, checkpoints,
    existing, host_path, listing, owned, owner, patch, probed, reference,
    reuse, subtree, text, worktree,
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
    assert_eq!(
        owned("", "def"),
        failure(
            "checkpoint: commit is not an immutable checkpoint of this workspace"
        )
    );
}

#[test]
fn a_checkpoint_id_taken_again_must_record_the_same_tree_and_parent() {
    assert_eq!(existing(failed("")), Ok(None));
    assert_eq!(existing(output(b"abc\n")), Ok(Some(String::from("abc"))));
    assert_eq!(reuse(("t", "p"), "t", "p"), Ok(()));
    assert_eq!(
        reuse(("t", "p"), "u", "p"),
        failure(
            "checkpoint id already records a different tree: metadata differs"
        )
    );
    assert_eq!(
        reuse(("t", "p"), "t", "q"),
        failure(
            "checkpoint id already records a different parent: metadata differs"
        )
    );
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
