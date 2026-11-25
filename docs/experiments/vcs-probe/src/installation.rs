use std::{
    fs,
    io::Write,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use git2::{Index, Oid, Repository, Transaction, WorktreeAddOptions};
use serde::{Deserialize, Serialize};
use vcs_probe_core::installation::{self, Observation, Preconditions, Step};

use crate::{fixture, recovery::target as reference, snapshot};

const BRANCH: &str = "refs/heads/session-install";
const CANDIDATE: &str = "refs/conductor/prepared/install-one";
const RECEIPT: &str = "refs/conductor/installed/install-one";

#[derive(Serialize, Deserialize)]
struct Intent {
    path: PathBuf,
    gitdir: PathBuf,
    inode: u64,
    base: String,
    candidate: String,
    head: Vec<u8>,
    original_index: Vec<u8>,
    candidate_index: Vec<u8>,
    files: Vec<Vec<u8>>,
    history: Vec<u8>,
}

pub fn persist(directory: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    fs::create_dir_all(directory)?;
    fs::File::open(directory.parent().context("journal has no parent")?)?
        .sync_all()?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist_noclobber(directory.join(name))?;
    fs::File::open(directory)?.sync_all()?;
    Ok(())
}

fn files(path: &Path) -> Result<Vec<Vec<u8>>> {
    ["tracked.txt", "new.txt", "private.tmp"]
        .iter()
        .map(|name| Ok(fs::read(path.join(name))?))
        .collect()
}

fn journal(repo: &Repository) -> PathBuf {
    repo.path().join("conductor-install")
}

fn candidate_index(repo: &Repository, candidate: Oid) -> Result<Vec<u8>> {
    let directory = tempfile::tempdir_in(repo.path())?;
    let path = directory.path().join("prepared-index");
    let mut index = Index::open(&path)?;
    index.read_tree(&repo.find_commit(candidate)?.tree()?)?;
    index.write()?;
    Ok(fs::read(path)?)
}

fn prepare(repo: &Repository, base: Oid) -> Result<Intent> {
    let path = repo.workdir().context("missing workspace")?;
    let candidate = snapshot::capture(path)?;
    let staged = repo.index()?.write_tree()?;
    ensure!(
        vcs_probe_core::preserves_staging(
            staged == repo.find_commit(base)?.tree_id(),
            staged == repo.find_commit(candidate)?.tree_id()
        ),
        "partial staging refuses installation"
    );
    let intent = Intent {
        path: fs::canonicalize(path)?,
        gitdir: fs::canonicalize(repo.path())?,
        inode: fs::metadata(path)?.ino(),
        base: base.to_string(),
        candidate: candidate.to_string(),
        head: fs::read(repo.path().join("HEAD"))?,
        original_index: fs::read(repo.path().join("index"))?,
        candidate_index: candidate_index(repo, candidate)?,
        files: files(path)?,
        history: fs::read(repo.commondir().join("logs").join(BRANCH))?,
    };
    repo.reference(CANDIDATE, candidate, false, "retain prepared snapshot")?;
    persist(&journal(repo), "intent.json", &serde_json::to_vec(&intent)?)?;
    Ok(intent)
}

fn fixture(root: &Path) -> Result<(Repository, Repository, Oid)> {
    let (source, base) = fixture::init(&root.join("source"))?;
    fixture::write(&source, "tracked.txt", "source staged\n")?;
    let mut staged = source.index()?;
    staged.add_path(Path::new("tracked.txt"))?;
    staged.write()?;
    fixture::write(&source, "tracked.txt", "source unstaged\n")?;
    let reference = source.reference(BRANCH, base, false, "session")?;
    source.worktree(
        "install",
        &root.join("session"),
        Some(WorktreeAddOptions::new().reference(Some(&reference))),
    )?;
    drop(reference);
    let session = Repository::open(root.join("session"))?;
    fixture::write(&session, "tracked.txt", "snapshot\n")?;
    fixture::write(&session, "new.txt", "new\n")?;
    fixture::write(&session, "private.tmp", "ignored private\n")?;
    Ok((source, session, base))
}

fn observation(repo: &Repository, intent: &Intent) -> Result<Observation> {
    let candidate = Oid::from_str(&intent.candidate)?;
    let path = repo.workdir().context("missing session")?;
    let actual_index = fs::read(repo.path().join("index"))?;
    let branch = repo.refname_to_id(BRANCH)?;
    let identity = [
        fs::canonicalize(path)? == intent.path,
        fs::canonicalize(repo.path())? == intent.gitdir,
        fs::metadata(path)?.ino() == intent.inode,
        fs::read(repo.path().join("HEAD"))? == intent.head,
    ];
    Ok(Observation {
        receipt: vcs_probe_core::classify_receipt(
            reference(repo, RECEIPT)?.map(|oid| oid == candidate),
        ),
        preconditions: Preconditions {
            owned: identity.iter().all(|value| *value),
            retained: reference(repo, CANDIDATE)? == Some(candidate),
            files_unchanged: files(path)? == intent.files,
        },
        index: installation::index(
            actual_index == intent.original_index,
            actual_index == intent.candidate_index,
        ),
        head: installation::head(
            branch == Oid::from_str(&intent.base)?,
            branch == candidate,
        ),
        progress: installation::progress(
            journal(repo).join("index-begun").try_exists()?,
            journal(repo).join("branch-begun").try_exists()?,
        ),
        history_unchanged: fs::read(
            repo.commondir().join("logs").join(BRANCH),
        )? == intent.history,
    })
}

fn index_lock(repo: &Repository) -> Result<tempfile::NamedTempFile> {
    let path = repo.path().join("index.lock");
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    Ok(tempfile::NamedTempFile::from_parts(
        file,
        tempfile::TempPath::try_from_path(path)?,
    ))
}

fn locked_refs(repo: &Repository) -> Result<Transaction<'_>> {
    let mut locked = repo.transaction()?;
    locked.lock_ref(BRANCH)?;
    locked.lock_ref("HEAD")?;
    locked.lock_ref(RECEIPT)?;
    Ok(locked)
}

fn apply(
    repo: &Repository,
    intent: &Intent,
    step: Step,
    index: &mut Option<tempfile::NamedTempFile>,
) -> Result<()> {
    match step {
        Step::BeginIndex => {
            persist(&journal(repo), "index-begun", b"install-one")
        }
        Step::BeginBranch => {
            persist(&journal(repo), "branch-begun", b"install-one")
        }
        Step::InstallIndex => {
            let mut held = index.take().context("missing owned index lock")?;
            held.write_all(&intent.candidate_index)?;
            held.as_file().sync_all()?;
            held.persist(repo.path().join("index"))?;
            fs::File::open(repo.path())?.sync_all()?;
            Ok(())
        }
        Step::PublishBranch
        | Step::RecordReceipt
        | Step::Completed
        | Step::Refuse => Ok(()),
    }
}

fn publish(
    transaction: &mut Transaction<'_>,
    intent: &Intent,
    step: Step,
) -> Result<()> {
    installation::publication(step, BRANCH, RECEIPT)
        .map(|name| -> Result<()> {
            transaction.set_target(
                name,
                Oid::from_str(&intent.candidate)?,
                Some(&fixture::signature()?),
                "snapshot install-one",
            )?;
            Ok(())
        })
        .transpose()?;
    Ok(())
}

fn resume(path: &Path) -> Result<Step> {
    let repo = Repository::open(path)?;
    let intent: Intent =
        serde_json::from_slice(&fs::read(journal(&repo).join("intent.json"))?)?;
    if reference(&repo, RECEIPT)? == Some(Oid::from_str(&intent.candidate)?) {
        return Ok(Step::Completed);
    }
    let mut transaction = locked_refs(&repo)?;
    let mut index = Some(index_lock(&repo)?);
    let step = installation::next(observation(&repo, &intent)?);
    apply(&repo, &intent, step, &mut index)?;
    publish(&mut transaction, &intent, step)?;
    transaction.commit()?;
    Ok(step)
}

fn expect(path: &Path, expected: Step) -> Result<()> {
    let actual = resume(path)?;
    ensure!(actual == expected, "expected {expected:?}, got {actual:?}");
    Ok(())
}

fn boundaries(root: &Path) -> Result<()> {
    let (source, session, base) = fixture(&root.join("install-boundaries"))?;
    let original_head = fs::read(source.path().join("HEAD"))?;
    let original_index = fs::read(source.path().join("index"))?;
    let original_file = fs::read(
        source
            .workdir()
            .context("source missing")?
            .join("tracked.txt"),
    )?;
    let intent = prepare(&session, base)?;
    drop(session);
    for step in [
        Step::BeginIndex,
        Step::InstallIndex,
        Step::BeginBranch,
        Step::PublishBranch,
        Step::RecordReceipt,
        Step::Completed,
        Step::Completed,
    ] {
        expect(&intent.path, step)?;
        ensure!(
            files(&intent.path)? == intent.files,
            "installation changed files"
        );
        ensure!(
            fs::read(intent.gitdir.join("HEAD"))? == intent.head,
            "installation rewrote symbolic HEAD"
        );
        ensure!(
            fs::read(source.path().join("HEAD"))? == original_head,
            "installation changed source HEAD"
        );
        ensure!(
            fs::read(source.path().join("index"))? == original_index,
            "installation changed source index"
        );
        ensure!(
            fs::read(
                source
                    .workdir()
                    .context("source missing")?
                    .join("tracked.txt")
            )? == original_file,
            "installation changed source files"
        );
    }
    let reopened = Repository::open(&intent.path)?;
    ensure!(
        reopened.index()?.write_tree()?
            == reopened
                .find_commit(Oid::from_str(&intent.candidate)?)?
                .tree_id(),
        "installed index differs from snapshot"
    );
    ensure!(
        reopened.refname_to_id(BRANCH)? == Oid::from_str(&intent.candidate)?,
        "session branch did not move"
    );
    fixture::write(&reopened, "new.txt", "later user edit\n")?;
    reopened.set_head_detached(base)?;
    expect(&intent.path, Step::Completed)?;
    ensure!(
        fs::read(intent.path.join("new.txt"))? == b"later user edit\n",
        "replay erased later work"
    );
    Ok(())
}

fn partial_staging(root: &Path) -> Result<()> {
    let (_, session, base) = fixture(&root.join("install-partial"))?;
    fixture::write(&session, "tracked.txt", "staged only\n")?;
    let mut index = session.index()?;
    index.add_path(Path::new("tracked.txt"))?;
    index.write()?;
    fixture::write(&session, "tracked.txt", "unstaged\n")?;
    let before = fs::read(session.path().join("index"))?;
    ensure!(prepare(&session, base).is_err(), "partial staging accepted");
    ensure!(
        fs::read(session.path().join("index"))? == before,
        "partial staging lost"
    );
    ensure!(
        session.refname_to_id(BRANCH)? == base,
        "refusal moved branch"
    );
    ensure!(
        reference(&session, CANDIDATE)?.is_none(),
        "refusal retained candidate"
    );
    Ok(())
}

fn fully_staged(root: &Path) -> Result<()> {
    let (_, session, base) = fixture(&root.join("install-fully-staged"))?;
    let mut index = session.index()?;
    index.add_all(["*"], git2::IndexAddOption::DEFAULT, None)?;
    index.write()?;
    let intent = prepare(&session, base)?;
    for step in [
        Step::BeginIndex,
        Step::InstallIndex,
        Step::BeginBranch,
        Step::PublishBranch,
        Step::RecordReceipt,
        Step::Completed,
    ] {
        expect(&intent.path, step)?;
    }
    ensure!(
        files(&intent.path)? == intent.files,
        "fully staged installation changed files"
    );
    Ok(())
}

fn interrupted_rewrite(root: &Path) -> Result<()> {
    let (_, session, base) = fixture(&root.join("install-rewrite"))?;
    let intent = prepare(&session, base)?;
    for step in [
        Step::BeginIndex,
        Step::InstallIndex,
        Step::BeginBranch,
        Step::PublishBranch,
    ] {
        expect(&intent.path, step)?;
    }
    session.reference(BRANCH, base, true, "external rewind")?;
    expect(&intent.path, Step::Refuse)?;
    ensure!(
        session.refname_to_id(BRANCH)? == base,
        "recovery overwrote rewind"
    );
    ensure!(
        fs::read(session.path().join("index"))? == intent.candidate_index,
        "recovery rolled back installed index"
    );
    Ok(())
}

fn external_changes(root: &Path) -> Result<()> {
    let (_, session, base) = fixture(&root.join("install-external"))?;
    let intent = prepare(&session, base)?;
    expect(&intent.path, Step::BeginIndex)?;
    fixture::write(&session, "new.txt", "external\n")?;
    expect(&intent.path, Step::Refuse)?;
    fixture::write(&session, "new.txt", "new\n")?;
    session.set_head_detached(base)?;
    expect(&intent.path, Step::Refuse)?;
    session.set_head(BRANCH)?;
    let mut index = session.index()?;
    index.add_path(Path::new("tracked.txt"))?;
    index.write()?;
    expect(&intent.path, Step::Refuse)?;
    ensure!(
        session.refname_to_id(BRANCH)? == base,
        "external change moved branch"
    );
    Ok(())
}

fn stale_lock(root: &Path) -> Result<()> {
    let (_, session, base) = fixture(&root.join("install-stale-lock"))?;
    let intent = prepare(&session, base)?;
    let path = session.path().join("index.lock");
    fs::write(&path, "unknown writer")?;
    ensure!(resume(&intent.path).is_err(), "stale lock stolen");
    ensure!(
        fs::read(&path)? == b"unknown writer",
        "foreign lock changed"
    );
    ensure!(
        session.refname_to_id(BRANCH)? == base,
        "lock refusal moved branch"
    );
    fs::remove_file(path)?;
    expect(&intent.path, Step::BeginIndex)?;
    Ok(())
}

fn missing_retention(root: &Path) -> Result<()> {
    let (_, session, base) = fixture(&root.join("install-retention"))?;
    let intent = prepare(&session, base)?;
    let mut walk = session.revwalk()?;
    walk.push_glob("refs/conductor/prepared/*")?;
    let reachable = walk.collect::<std::result::Result<Vec<_>, _>>()?;
    ensure!(
        reachable.contains(&Oid::from_str(&intent.candidate)?),
        "prepared object is unreachable"
    );
    session.find_reference(CANDIDATE)?.delete()?;
    expect(&intent.path, Step::Refuse)?;
    Ok(())
}

fn locks_do_not_guard_paths(root: &Path) -> Result<()> {
    let (_, session, base) = fixture(&root.join("install-path-race"))?;
    let intent = prepare(&session, base)?;
    let mut branch_lock = session.transaction()?;
    branch_lock.lock_ref(BRANCH)?;
    let other = Repository::open(&intent.path)?;
    other.set_head_detached(base)?;
    ensure!(
        other.head_detached()?,
        "branch lock unexpectedly guarded HEAD"
    );
    other.set_head(BRANCH)?;
    let index = index_lock(&session)?;
    let moved = intent.path.with_extension("moved");
    fs::rename(&intent.path, &moved)?;
    fs::create_dir(&intent.path)?;
    fs::write(
        intent.path.join(".git"),
        format!("gitdir: {}\n", intent.gitdir.display()),
    )?;
    fs::copy(moved.join("tracked.txt"), intent.path.join("tracked.txt"))?;
    fs::copy(moved.join("new.txt"), intent.path.join("new.txt"))?;
    fs::copy(moved.join("private.tmp"), intent.path.join("private.tmp"))?;
    drop(index);
    drop(branch_lock);
    expect(&intent.path, Step::Refuse)?;
    ensure!(
        fs::read(moved.join("new.txt"))? == b"new\n",
        "path refusal changed original files"
    );
    Ok(())
}

pub fn run(root: &Path) -> Result<()> {
    boundaries(root)?;
    partial_staging(root)?;
    fully_staged(root)?;
    interrupted_rewrite(root)?;
    external_changes(root)?;
    stale_lock(root)?;
    missing_retention(root)?;
    locks_do_not_guard_paths(root)?;
    Ok(())
}
