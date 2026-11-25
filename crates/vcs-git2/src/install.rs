use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::Path;

use anyhow::{Context as _, Result, ensure};
use git_workspace::install::{
    Head, Index as IndexState, Observation, Progress, Step,
};
use git2::{Index, Oid, Reference, Repository, Signature, Transaction};
use tempfile::{NamedTempFile, TempPath};

mod journal;
mod observed;

use journal::{INDEX_LIMIT, Intent, Journal};
use observed::{Checkout, oid};

pub fn ensure_available(
    repo: &Repository,
    attempt: Option<&str>,
) -> Result<(), vcs::Error> {
    available(repo, attempt).map_err(super::failure)
}

fn available(repo: &Repository, attempt: Option<&str>) -> Result<()> {
    let root = repo.path().join("orchestrator-install");
    if let Some(active) = journal::optional(&root.join("active"), 64)? {
        let key = attempt
            .map(|value| journal::scope(repo, value))
            .transpose()?;
        ensure!(
            key.as_ref().is_some_and(|value| value.as_bytes() == active),
            "an unfinished snapshot installation owns this workspace"
        );
    }
    Ok(())
}

pub fn recorded(
    repo: &Repository,
    branch: &str,
    attempt: &str,
) -> Result<Option<Oid>, vcs::Error> {
    previous(repo, branch, attempt)
        .map_err(|error| outcome(repo, attempt, error))
}

fn previous(
    repo: &Repository,
    branch: &str,
    attempt: &str,
) -> Result<Option<Oid>> {
    let journal = Journal::new(repo, attempt)?;
    if journal.directory.try_exists()? {
        let intent = journal.load(branch)?;
        return Ok(Some(oid(&intent.candidate)?));
    }
    ensure!(
        Journal::target(repo, &journal.receipt())?.is_none(),
        "snapshot receipt has lost its intent"
    );
    available(repo, Some(attempt))?;
    ensure!(
        journal::optional(&journal.root.join("active"), 64)?.is_none(),
        "snapshot preparation was interrupted; reconcile its intent"
    );
    Ok(None)
}

pub fn run(
    repo: &Repository,
    branch: &str,
    attempt: &str,
    candidate: Oid,
) -> Result<(), vcs::Error> {
    install(repo, branch, attempt, candidate)
        .map_err(|error| outcome(repo, attempt, error))
}

fn outcome(
    repo: &Repository,
    attempt: &str,
    error: anyhow::Error,
) -> vcs::Error {
    let evidence = has_evidence(repo, attempt);
    if evidence.unwrap_or(true) {
        return vcs::Error::Reconciliation {
            attempt: attempt.to_owned(),
            reason: format!("{error:#}"),
        };
    }
    super::failure(error)
}

fn has_evidence(repo: &Repository, attempt: &str) -> Result<bool> {
    let journal = Journal::new(repo, attempt)?;
    let active = journal::optional(&journal.root.join("active"), 64)?;
    Ok(journal.directory.try_exists()?
        || active.as_deref() == Some(journal.key.as_bytes())
        || Journal::target(repo, &journal.receipt())?.is_some())
}

fn install(
    repo: &Repository,
    branch: &str,
    attempt: &str,
    candidate: Oid,
) -> Result<()> {
    let journal = Journal::new(repo, attempt)?;
    let prior = previous(repo, branch, attempt)?;
    ensure!(
        prior.is_none_or(|revision| revision == candidate),
        "snapshot installation identity reused for different work"
    );
    if completed(repo, &journal, candidate)? {
        return journal.release();
    }
    available(repo, Some(attempt))?;
    if prior.is_none() {
        prepare(repo, &journal, branch, candidate)?;
    }
    let intent = journal.load(branch)?;
    finish(repo, &intent, &journal)
}

fn finish(repo: &Repository, intent: &Intent, journal: &Journal) -> Result<()> {
    while !advance(repo, intent, journal)? {}
    journal.release()
}

fn completed(
    repo: &Repository,
    journal: &Journal,
    candidate: Oid,
) -> Result<bool> {
    let held = Journal::target(repo, &journal.receipt())?;
    ensure!(
        held.is_none_or(|found| found == candidate),
        "snapshot receipt disagrees with request"
    );
    if held.is_some() {
        verify_objects(repo, candidate)?;
    }
    Ok(held.is_some())
}

fn prepare(
    repo: &Repository,
    journal: &Journal,
    branch: &str,
    candidate: Oid,
) -> Result<()> {
    let mut refs = locked(repo, branch, &journal.pin())?;
    let _index = index_lock(repo)?;
    let base = validate_head(repo, branch)?;
    let commit = repo.find_commit(candidate)?;
    ensure!(
        candidate == base
            || (commit.parent_count() == 1 && commit.parent_id(0)? == base),
        "snapshot was captured on another revision"
    );
    super::features::check(repo, base)?;
    super::features::check(repo, candidate)?;
    let checkout = Checkout::observe(repo)?;
    ensure!(
        super::snapshot::tree(repo)? == commit.tree_id(),
        "workspace changed since snapshot capture"
    );
    let (original, next) = indexes(repo, base, &commit)?;
    let (history, history_length) = observed::history(repo, branch)?;
    let intent = Intent {
        version: 1,
        key: journal.key.clone(),
        checkout,
        original_index: hash(&original),
        candidate_index: hash(&next),
        branch: branch.to_owned(),
        base: base.to_string(),
        candidate: candidate.to_string(),
        history,
        history_length,
    };
    ensure!(
        Journal::target(repo, &journal.pin())?.is_none(),
        "prepared snapshot pin already exists"
    );
    journal.reserve(&intent, &next)?;
    refs.set_target(
        &journal.pin(),
        candidate,
        Some(&signature()?),
        "retain snapshot installation",
    )?;
    refs.commit()?;
    sync_ref(repo, &journal.pin())
}

fn sync_ref(repo: &Repository, name: &str) -> Result<()> {
    let common = repo.commondir();
    let path = common.join(name);
    journal::sync(&path)?;
    for directory in path
        .ancestors()
        .skip(1)
        .take_while(|directory| directory.starts_with(common))
    {
        journal::sync(directory)?;
    }
    Ok(())
}

fn verify_objects(repo: &Repository, candidate: Oid) -> Result<()> {
    let commit = repo.find_commit(candidate)?;
    let tree = commit.tree()?;
    verify_tree(repo, &tree)
}

fn verify_tree(repo: &Repository, tree: &git2::Tree<'_>) -> Result<()> {
    for entry in tree {
        let object = entry.to_object(repo)?;
        if let Some(child) = object.as_tree() {
            verify_tree(repo, child)?;
        }
    }
    Ok(())
}

fn indexes(
    repo: &Repository,
    base: Oid,
    commit: &git2::Commit<'_>,
) -> Result<(Vec<u8>, Vec<u8>)> {
    let staged = repo.index()?.write_tree()?;
    let previous_tree = repo.find_commit(base)?.tree_id();
    ensure!(
        staged == previous_tree || staged == commit.tree_id(),
        "snapshot would discard partial staging"
    );
    let original = journal::read(&repo.path().join("index"), INDEX_LIMIT)?;
    let next = if staged == commit.tree_id() {
        original.clone()
    } else {
        candidate_index(repo, commit.id())?
    };
    Ok((original, next))
}

fn signature() -> Result<Signature<'static>> {
    Ok(Signature::now("Conductor", "conductor@local")?)
}

fn hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn validate_head(repo: &Repository, branch: &str) -> Result<Oid> {
    ensure!(
        branch.starts_with("refs/heads/") && Reference::is_valid_name(branch),
        "invalid owned workspace branch"
    );
    ensure!(
        repo.find_reference("HEAD")?.symbolic_target()? == Some(branch),
        "workspace HEAD no longer names its owned branch"
    );
    Ok(repo.refname_to_id(branch)?)
}

fn candidate_index(repo: &Repository, candidate: Oid) -> Result<Vec<u8>> {
    let tmp = tempfile::tempdir_in(repo.path())?;
    let path = tmp.path().join("candidate-index");
    let mut index = Index::open(&path)?;
    index.read_tree(&repo.find_commit(candidate)?.tree()?)?;
    index.write()?;
    journal::read(&path, INDEX_LIMIT)
}

fn index_lock(repo: &Repository) -> Result<NamedTempFile> {
    let path = repo.path().join("index.lock");
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .context("acquiring workspace index.lock")?;
    Ok(NamedTempFile::from_parts(
        file,
        TempPath::try_from_path(path)?,
    ))
}

fn locked<'a>(
    repo: &'a Repository,
    branch: &str,
    extra: &str,
) -> Result<Transaction<'a>> {
    ensure!(
        branch.starts_with("refs/heads/") && Reference::is_valid_name(branch),
        "invalid installation branch"
    );
    let mut refs = repo.transaction()?;
    refs.lock_ref(branch)?;
    refs.lock_ref("HEAD")?;
    refs.lock_ref(extra)?;
    Ok(refs)
}

fn progress(journal: &Journal) -> Result<Progress> {
    match (
        journal.marker("index-started")?,
        journal.marker("branch-started")?,
    ) {
        (false, false) => Ok(Progress::Prepared),
        (true, false) => Ok(Progress::IndexStarted),
        (true, true) => Ok(Progress::BranchStarted),
        (false, true) => {
            anyhow::bail!("inconsistent snapshot installation progress")
        }
    }
}

fn head(
    repo: &Repository,
    intent: &Intent,
    journal: &Journal,
    actual: Oid,
) -> Result<Head> {
    let baseline = observed::history(repo, &intent.branch)?;
    let original = baseline == (intent.history.clone(), intent.history_length);
    let base = oid(&intent.base)?;
    let candidate = oid(&intent.candidate)?;
    Ok(match (actual == base, actual == candidate, original) {
        (true, true, true) => Head::Unchanged,
        (true, false, true) => Head::Original,
        (false, true, false) if observed::published(repo, intent, journal)? => {
            Head::Published
        }
        _ => Head::Changed,
    })
}

fn observe(
    repo: &Repository,
    intent: &Intent,
    journal: &Journal,
) -> Result<Observation> {
    let current = validate_head(repo, &intent.branch)?;
    let candidate = oid(&intent.candidate)?;
    super::features::check(repo, current)?;
    ensure!(
        Journal::target(repo, &journal.pin())? == Some(candidate),
        "prepared snapshot retention changed; reconcile the installation"
    );
    let actual = hash(&journal::read(&repo.path().join("index"), INDEX_LIMIT)?);
    let index = match (
        actual == intent.original_index,
        actual == intent.candidate_index,
    ) {
        (_, true) => IndexState::Candidate,
        (true, false) => IndexState::Original,
        _ => IndexState::Changed,
    };
    Ok(Observation {
        progress: progress(journal)?,
        head: head(repo, intent, journal, current)?,
        index,
        unchanged: intent.checkout == Checkout::observe(repo)?,
    })
}

fn advance(
    repo: &Repository,
    intent: &Intent,
    journal: &Journal,
) -> Result<bool> {
    ensure!(
        journal::read(&journal.root.join("active"), 64)?
            == journal.key.as_bytes(),
        "active install reservation changed"
    );
    let mut refs = locked(repo, &intent.branch, &journal.receipt())?;
    refs.lock_ref(&journal.pin())?;
    let index = index_lock(repo)?;
    let step = observe(repo, intent, journal)?.next();
    match step {
        Step::BeginIndex => journal.advance("index-started")?,
        Step::WriteIndex => replace_index(index, repo.path(), intent, journal)?,
        Step::BeginBranch => journal.advance("branch-started")?,
        Step::WriteBranch | Step::Receipt => {
            let name = publication(step, intent, journal);
            refs.set_target(
                &name,
                oid(&intent.candidate)?,
                Some(&signature()?),
                &journal.message(),
            )?;
            refs.commit()?;
            sync_ref(repo, &name)?;
        }
        Step::Refuse => anyhow::bail!(
            "workspace or branch changed; reconcile the unfinished snapshot installation"
        ),
    }
    Ok(step == Step::Receipt)
}

fn publication(step: Step, intent: &Intent, journal: &Journal) -> String {
    if step == Step::Receipt {
        journal.receipt()
    } else {
        intent.branch.clone()
    }
}

fn replace_index(
    mut lock: NamedTempFile,
    gitdir: &Path,
    intent: &Intent,
    journal: &Journal,
) -> Result<()> {
    let bytes = journal::read(&journal.directory.join("index"), INDEX_LIMIT)?;
    ensure!(
        hash(&bytes) == intent.candidate_index,
        "prepared index checksum differs"
    );
    let destination = gitdir.join("index");
    lock.as_file()
        .set_permissions(fs::metadata(&destination)?.permissions())?;
    lock.write_all(&bytes)?;
    lock.as_file().sync_all()?;
    lock.persist(destination)?;
    journal::sync(gitdir)
}

#[cfg(test)]
mod tests;
