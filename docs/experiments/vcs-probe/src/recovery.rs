use std::{fs, io::Write, path::Path};

use anyhow::{Context, Result, ensure};
use git2::{ErrorCode, Oid, Repository, Transaction};
use serde::{Deserialize, Serialize};
use vcs_probe_core::{Branch, Receipt, Recovery};

use crate::fixture;

const BRANCH: &str = "refs/heads/issue";
const RECEIPT: &str = "refs/conductor/land/attempt-one";
const CANDIDATE: &str = "refs/conductor/prepared/land-attempt-one";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Prepared {
    branch: String,
    receipt: String,
    expected: String,
    candidate: String,
    added_count: usize,
    baseline_log: Vec<u8>,
}

struct Progress {
    action: Recovery,
    added_count: usize,
}

fn lock<'a>(
    repo: &'a Repository,
    branch: &str,
    receipt: &str,
) -> Result<Transaction<'a>> {
    let mut transaction = repo.transaction()?;
    transaction.lock_ref(branch)?;
    transaction.lock_ref(receipt)?;
    Ok(transaction)
}

fn save(repo: &Repository, prepared: &Prepared) -> Result<()> {
    let directory = repo.commondir().join("conductor-land");
    fs::create_dir_all(&directory)?;
    let mut file = tempfile::NamedTempFile::new_in(&directory)?;
    file.write_all(&serde_json::to_vec(prepared)?)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(directory.join("attempt-one.json"))?;
    fs::File::open(directory)?.sync_all()?;
    Ok(())
}

fn load(repo: &Repository) -> Result<Prepared> {
    Ok(serde_json::from_slice(&fs::read(
        repo.commondir().join("conductor-land/attempt-one.json"),
    )?)?)
}

fn prepare(path: &Path) -> Result<Prepared> {
    let (repo, base) = fixture::init(path)?;
    let first = fixture::commit(
        &repo,
        Some(base),
        &[("first.txt", "first\n")],
        "first session commit",
    )?;
    let candidate = fixture::commit(
        &repo,
        Some(first),
        &[("second.txt", "second\n")],
        "second session commit",
    )?;
    repo.reference(BRANCH, base, false, "issue branch")?;
    repo.reference_ensure_log(BRANCH)?;
    let transaction = lock(&repo, BRANCH, RECEIPT)?;
    let mut walk = repo.revwalk()?;
    walk.push(candidate)?;
    walk.hide(base)?;
    let commits: Vec<_> = walk.collect::<std::result::Result<_, _>>()?;
    let prepared = Prepared {
        branch: BRANCH.to_owned(),
        receipt: RECEIPT.to_owned(),
        expected: base.to_string(),
        candidate: candidate.to_string(),
        added_count: commits.len(),
        baseline_log: fs::read(repo.commondir().join("logs").join(BRANCH))?,
    };
    repo.reference(
        CANDIDATE,
        candidate,
        false,
        "retain integration candidate",
    )?;
    save(&repo, &prepared)?;
    drop(transaction);
    Ok(prepared)
}

fn receipt(repo: &Repository, prepared: &Prepared) -> Result<Receipt> {
    let candidate = Oid::from_str(&prepared.candidate)?;
    Ok(vcs_probe_core::classify_receipt(
        target(repo, &prepared.receipt)?.map(|oid| oid == candidate),
    ))
}

pub fn target(repo: &Repository, name: &str) -> Result<Option<Oid>> {
    repo.references_glob(name)?
        .next()
        .transpose()?
        .map(|reference| {
            reference.target().context("expected a direct owned ref")
        })
        .transpose()
}

fn branch(repo: &Repository, prepared: &Prepared) -> Result<Branch> {
    let current = repo.refname_to_id(&prepared.branch)?;
    let candidate = Oid::from_str(&prepared.candidate)?;
    Ok(vcs_probe_core::classify_branch(
        current == Oid::from_str(&prepared.expected)?,
        current == candidate,
        repo.graph_descendant_of(current, candidate)?,
    ))
}

fn checked_out(repo: &Repository, name: &str) -> Result<bool> {
    let main = repo.head()?.name()? == name;
    let worktrees = repo.worktrees()?;
    let names = worktrees
        .iter()
        .map(|entry| entry?.context("invalid worktree name"));
    let heads: Vec<_> = names
        .map(|entry| {
            let worktree = repo.find_worktree(entry?)?;
            Ok(Repository::open(worktree.path())?.head()?.name()? == name)
        })
        .collect::<Result<_>>()?;
    Ok(vcs_probe_core::any_checkout(main, &heads))
}

fn history_unchanged(repo: &Repository, prepared: &Prepared) -> Result<bool> {
    fs::read(repo.commondir().join("logs").join(&prepared.branch))
        .map(|current| current == prepared.baseline_log)
        .or_else(|error| {
            (error.kind() == std::io::ErrorKind::NotFound)
                .then_some(false)
                .ok_or(error)
        })
        .map_err(Into::into)
}

fn observe(repo: &Repository, prepared: &Prepared) -> Result<Recovery> {
    let observed_receipt = receipt(repo, prepared)?;
    if observed_receipt == Receipt::Matches {
        return Ok(Recovery::Completed);
    }
    Ok(vcs_probe_core::guard_retention(
        vcs_probe_core::recover(
            observed_receipt,
            branch(repo, prepared)?,
            history_unchanged(repo, prepared)?,
        ),
        target(repo, CANDIDATE)? == Some(Oid::from_str(&prepared.candidate)?),
    ))
}

fn resume(path: &Path) -> Result<Progress> {
    let repo = Repository::open(path)?;
    let prepared = load(&repo)?;
    let mut transaction = lock(&repo, &prepared.branch, &prepared.receipt)?;
    let action = next_action(&repo, &prepared)?;
    vcs_probe_core::recovery_target(
        action,
        &prepared.branch,
        &prepared.receipt,
    )
    .map(|name| -> Result<()> {
        transaction.set_target(
            name,
            Oid::from_str(&prepared.candidate)?,
            Some(&fixture::signature()?),
            "landing attempt-one",
        )?;
        Ok(())
    })
    .transpose()?;
    transaction.commit()?;
    Ok(Progress {
        action,
        added_count: prepared.added_count,
    })
}

fn next_action(repo: &Repository, prepared: &Prepared) -> Result<Recovery> {
    let action = observe(repo, prepared)?;
    let occupied = vcs_probe_core::checkout_branch(action, &prepared.branch)
        .map(|name| checked_out(repo, name))
        .transpose()?
        .unwrap_or(false);
    Ok(vcs_probe_core::guard_checkout(action, occupied))
}

fn expect_step(path: &Path, expected: Recovery) -> Result<()> {
    let progress = resume(path)?;
    ensure!(
        progress.action == expected,
        "expected {expected:?}, got {:?}",
        progress.action
    );
    ensure!(progress.added_count == 2, "landing count changed on retry");
    Ok(())
}

fn completed(path: &Path, prepared: &Prepared) -> Result<()> {
    let repo = Repository::open(path)?;
    let candidate = Oid::from_str(&prepared.candidate)?;
    ensure!(
        repo.refname_to_id(BRANCH)? == candidate,
        "landing branch did not reach candidate"
    );
    ensure!(
        repo.refname_to_id(RECEIPT)? == candidate,
        "landing receipt did not reach candidate"
    );
    ensure!(
        repo.reflog(BRANCH)?.len() == 2,
        "landing branch was updated more than once"
    );
    Ok(())
}

fn interrupted_boundaries(root: &Path) -> Result<()> {
    let path = root.join("interrupted-boundaries");
    let prepared = prepare(&path)?;
    expect_step(&path, Recovery::Publish)?;
    expect_step(&path, Recovery::RepairReceipt)?;
    expect_step(&path, Recovery::Completed)?;
    expect_step(&path, Recovery::Completed)?;
    completed(&path, &prepared)?;
    Ok(())
}

fn move_branch(path: &Path, target: Oid) -> Result<()> {
    Repository::open(path)?.reference(
        BRANCH,
        target,
        true,
        "competing writer",
    )?;
    Ok(())
}

fn advance(path: &Path, parent: &str) -> Result<Oid> {
    let repo = Repository::open(path)?;
    let commit = fixture::commit(
        &repo,
        Some(Oid::from_str(parent)?),
        &[("external.txt", "external\n")],
        "external",
    )?;
    move_branch(path, commit)?;
    Ok(commit)
}

fn divergent(root: &Path) -> Result<()> {
    let path = root.join("divergent");
    let prepared = prepare(&path)?;
    let external = advance(&path, &prepared.expected)?;
    expect_step(&path, Recovery::Refuse)?;
    ensure!(
        Repository::open(&path)?.refname_to_id(BRANCH)? == external,
        "diverged branch was overwritten"
    );
    Ok(())
}

fn descendant(root: &Path) -> Result<()> {
    let path = root.join("descendant");
    let prepared = prepare(&path)?;
    expect_step(&path, Recovery::Publish)?;
    let external = advance(&path, &prepared.candidate)?;
    expect_step(&path, Recovery::RepairReceipt)?;
    expect_step(&path, Recovery::Completed)?;
    ensure!(
        Repository::open(&path)?.refname_to_id(BRANCH)? == external,
        "receipt repair rewound a descendant"
    );
    Ok(())
}

fn rewound(root: &Path) -> Result<()> {
    let path = root.join("rewound");
    let prepared = prepare(&path)?;
    expect_step(&path, Recovery::Publish)?;
    let expected = Oid::from_str(&prepared.expected)?;
    move_branch(&path, expected)?;
    expect_step(&path, Recovery::Refuse)?;
    ensure!(
        Repository::open(&path)?.refname_to_id(BRANCH)? == expected,
        "uncertain landing was blindly reapplied"
    );
    Ok(())
}

fn completed_rewrite(root: &Path) -> Result<()> {
    let path = root.join("completed-rewrite");
    let prepared = prepare(&path)?;
    expect_step(&path, Recovery::Publish)?;
    expect_step(&path, Recovery::RepairReceipt)?;
    let expected = Oid::from_str(&prepared.expected)?;
    move_branch(&path, expected)?;
    expect_step(&path, Recovery::Completed)?;
    ensure!(
        Repository::open(&path)?.refname_to_id(BRANCH)? == expected,
        "completed retry changed rewritten branch"
    );
    let repo = Repository::open(&path)?;
    repo.find_reference(BRANCH)?.delete()?;
    repo.set_head("refs/heads/missing")?;
    expect_step(&path, Recovery::Completed)?;
    ensure!(
        target(&repo, BRANCH)?.is_none(),
        "completed retry recreated deleted branch"
    );
    Ok(())
}

fn wrong_receipt(root: &Path) -> Result<()> {
    let path = root.join("wrong-receipt");
    let prepared = prepare(&path)?;
    let repo = Repository::open(&path)?;
    repo.reference(
        RECEIPT,
        Oid::from_str(&prepared.expected)?,
        false,
        "wrong receipt",
    )?;
    expect_step(&path, Recovery::Refuse)?;
    ensure!(
        repo.refname_to_id(BRANCH)? == Oid::from_str(&prepared.expected)?,
        "wrong receipt changed branch"
    );
    Ok(())
}

fn missing_history(root: &Path) -> Result<()> {
    let path = root.join("missing-history");
    prepare(&path)?;
    let repo = Repository::open(&path)?;
    repo.reflog_delete(BRANCH)?;
    expect_step(&path, Recovery::Refuse)?;
    Ok(())
}

fn lock_contention(root: &Path) -> Result<()> {
    let path = root.join("lock-contention");
    let prepared = prepare(&path)?;
    let repo = Repository::open(&path)?;
    let held = lock(&repo, BRANCH, RECEIPT)?;
    ensure!(
        resume(&path).is_err(),
        "competing landing bypassed held ref lock"
    );
    let another = Repository::open(&path)?;
    let rejected = another.reference_matching(
        BRANCH,
        Oid::from_str(&prepared.candidate)?,
        true,
        Oid::from_str(&prepared.expected)?,
        "competing writer",
    );
    ensure!(
        matches!(rejected, Err(error) if error.code() == ErrorCode::Locked),
        "writer bypassed landing's branch lock"
    );
    drop(held);
    expect_step(&path, Recovery::Publish)?;
    Ok(())
}

fn intent_collision(root: &Path) -> Result<()> {
    let path = root.join("intent-collision");
    let prepared = prepare(&path)?;
    let repo = Repository::open(&path)?;
    let mut changed = prepared.clone();
    changed.added_count = 99;
    ensure!(
        save(&repo, &changed).is_err(),
        "existing attempt was replaced"
    );
    ensure!(load(&repo)? == prepared, "attempt changed after collision");
    Ok(())
}

fn checked_out_branch(root: &Path) -> Result<()> {
    let path = root.join("checked-out");
    let prepared = prepare(&path)?;
    let repo = Repository::open(&path)?;
    let reference = repo.find_reference(BRANCH)?;
    let mut options = git2::WorktreeAddOptions::new();
    options.reference(Some(&reference));
    repo.worktree(
        "issue-worktree",
        &root.join("issue-worktree"),
        Some(&options),
    )?;
    expect_step(&path, Recovery::Refuse)?;
    ensure!(
        repo.refname_to_id(BRANCH)? == Oid::from_str(&prepared.expected)?,
        "landing changed a checked-out branch"
    );
    Ok(())
}

fn retained_candidate(root: &Path) -> Result<()> {
    let path = root.join("integration-retention");
    let prepared = prepare(&path)?;
    let repo = Repository::open(&path)?;
    let mut walk = repo.revwalk()?;
    walk.push_glob("refs/conductor/prepared/*")?;
    let retained = walk.collect::<std::result::Result<Vec<_>, _>>()?;
    ensure!(
        retained.contains(&Oid::from_str(&prepared.candidate)?),
        "integration candidate is unreachable"
    );
    repo.find_reference(CANDIDATE)?.delete()?;
    expect_step(&path, Recovery::Refuse)?;
    ensure!(
        repo.refname_to_id(BRANCH)? == Oid::from_str(&prepared.expected)?,
        "missing retention changed branch"
    );
    Ok(())
}

pub fn run(root: &Path) -> Result<()> {
    interrupted_boundaries(root)?;
    divergent(root)?;
    descendant(root)?;
    rewound(root)?;
    completed_rewrite(root)?;
    wrong_receipt(root)?;
    missing_history(root)?;
    lock_contention(root)?;
    intent_collision(root)?;
    checked_out_branch(root)?;
    retained_candidate(root)?;
    Ok(())
}
