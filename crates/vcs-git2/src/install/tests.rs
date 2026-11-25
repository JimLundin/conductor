use super::{
    Journal, advance, ensure_available, install, prepare, recorded, run,
};
use anyhow::{Context as _, Result};
use git2::{Oid, Repository, Signature, WorktreeAddOptions};
use std::fs;
use std::path::Path;

const BRANCH: &str = "refs/heads/session";
const ATTEMPT: &str = "session-001/turn-2";

struct Fixture {
    _root: tempfile::TempDir,
    source: Repository,
    session: Repository,
    base: Oid,
    captured: Oid,
}

impl Fixture {
    fn new() -> Result<Self> {
        let root = tempfile::tempdir()?;
        let source_path = root.path().join("source");
        let source = Repository::init(&source_path)?;
        source.config()?.set_bool("core.logallrefupdates", true)?;
        fs::write(source_path.join("file"), "base")?;
        fs::write(source_path.join(".gitignore"), "*.tmp\n")?;
        let mut index = source.index()?;
        index.add_path(Path::new("file"))?;
        index.add_path(Path::new(".gitignore"))?;
        index.write()?;
        let tree = index.write_tree()?;
        drop(index);
        let signature = Signature::now("Fixture", "fixture@example.com")?;
        let base = source.commit(
            Some("HEAD"),
            &signature,
            &signature,
            "base",
            &source.find_tree(tree)?,
            &[],
        )?;
        let branch = source.reference(BRANCH, base, false, "own workspace")?;
        let path = root.path().join("session");
        source.worktree(
            "session",
            &path,
            Some(WorktreeAddOptions::new().reference(Some(&branch))),
        )?;
        drop(branch);
        let session = Repository::open(&path)?;
        fs::write(path.join("file"), "record this")?;
        fs::write(path.join("private.tmp"), "ignored")?;
        let captured = crate::snapshot::candidate(&session)?;
        Ok(Self {
            _root: root,
            source,
            session,
            base,
            captured,
        })
    }

    fn path(&self) -> Result<&Path> {
        self.session.workdir().context("owned workspace")
    }

    fn start(&self) -> Result<Journal> {
        let journal = Journal::new(&self.session, ATTEMPT)?;
        prepare(&self.session, &journal, BRANCH, self.captured)?;
        Ok(journal)
    }
}

#[test]
fn resumed_installation_uses_the_reserved_revision_and_preserves_both_checkouts()
-> Result<()> {
    let fixture = Fixture::new()?;
    let source_index = fs::read(fixture.source.path().join("index"))?;
    let session_head = fs::read(fixture.session.path().join("HEAD"))?;
    let journal = fixture.start()?;
    assert!(ensure_available(&fixture.session, Some("another turn")).is_err());
    assert!(ensure_available(&fixture.session, None).is_err());
    for _ in 0..5 {
        let reopened = Repository::open(fixture.path()?)?;
        assert_eq!(
            recorded(&reopened, BRANCH, ATTEMPT)?,
            Some(fixture.captured)
        );
        let intent = journal.load(BRANCH)?;
        advance(&reopened, &intent, &journal)?;
    }
    run(&fixture.session, BRANCH, ATTEMPT, fixture.captured)?;
    ensure_available(&fixture.session, None)?;
    assert_eq!(fixture.session.refname_to_id(BRANCH)?, fixture.captured);
    assert_eq!(fs::read(fixture.session.path().join("HEAD"))?, session_head);
    assert_eq!(fs::read(fixture.source.path().join("index"))?, source_index);
    assert_eq!(fs::read(fixture.path()?.join("file"))?, b"record this");
    assert_eq!(fs::read(fixture.path()?.join("private.tmp"))?, b"ignored");
    fs::write(fixture.path()?.join("file"), "later edit")?;
    fixture.session.set_head_detached(fixture.base)?;
    run(&fixture.session, BRANCH, ATTEMPT, fixture.captured)?;
    assert!(run(&fixture.session, BRANCH, ATTEMPT, fixture.base).is_err());
    assert_eq!(fs::read(fixture.path()?.join("file"))?, b"later edit");
    Ok(())
}

#[test]
fn changed_ignored_file_blocks_an_interrupted_installation_without_writing_the_index()
-> Result<()> {
    let fixture = Fixture::new()?;
    let journal = fixture.start()?;
    advance(&fixture.session, &journal.load(BRANCH)?, &journal)?;
    let before = fs::read(fixture.session.path().join("index"))?;
    fs::write(fixture.path()?.join("private.tmp"), "somebody else")?;
    assert!(
        install(&fixture.session, BRANCH, ATTEMPT, fixture.captured).is_err()
    );
    assert_eq!(fs::read(fixture.session.path().join("index"))?, before);
    assert_eq!(fixture.session.refname_to_id(BRANCH)?, fixture.base);
    assert_eq!(
        Journal::target(&fixture.session, &journal.pin())?,
        Some(fixture.captured)
    );
    Ok(())
}

#[test]
fn partial_staging_is_refused_before_reserving_the_workspace() -> Result<()> {
    let fixture = Fixture::new()?;
    fs::write(fixture.path()?.join("file"), "only staged")?;
    let mut index = fixture.session.index()?;
    index.add_path(Path::new("file"))?;
    index.write()?;
    fs::write(fixture.path()?.join("file"), "record this")?;
    let before = fs::read(fixture.session.path().join("index"))?;
    assert!(
        install(&fixture.session, BRANCH, ATTEMPT, fixture.captured).is_err()
    );
    assert_eq!(fs::read(fixture.session.path().join("index"))?, before);
    assert_eq!(recorded(&fixture.session, BRANCH, ATTEMPT)?, None);
    ensure_available(&fixture.session, None)?;
    Ok(())
}

#[cfg(target_os = "linux")]
const CRASH_FIXTURE: &str = "CONDUCTOR_TEST_INSTALL_CRASH_WORKTREE";

#[cfg(target_os = "linux")]
#[test]
fn process_death_after_installing_the_index_resumes_the_same_snapshot()
-> Result<()> {
    use std::io::{BufRead as _, BufReader};
    use std::os::unix::process::ExitStatusExt as _;
    use std::process::Command;

    if let Some(path) = std::env::var_os(CRASH_FIXTURE) {
        return crash_after_index(&path);
    }
    let fixture = Fixture::new()?;
    let _journal = fixture.start()?;
    let mut child = Command::new(std::env::current_exe()?)
        .args(["--exact", "install::tests::process_death_after_installing_the_index_resumes_the_same_snapshot", "--nocapture"])
        .env(CRASH_FIXTURE, fixture.path()?)
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    let output =
        BufReader::new(child.stdout.take().context("crash fixture stdout")?);
    let boundary = output
        .lines()
        .map_while(Result::ok)
        .any(|line| line.contains("INDEX_INSTALLED"));
    if boundary {
        child.kill()?;
    }
    let status = child.wait()?;
    assert!(boundary, "child did not reach the persisted index boundary");
    assert_eq!(status.signal(), Some(9));
    assert_eq!(fixture.session.refname_to_id(BRANCH)?, fixture.base);
    let retention = Journal::new(&fixture.session, ATTEMPT)?;
    assert_eq!(
        Journal::target(&fixture.session, &retention.pin())?,
        Some(fixture.captured)
    );
    let retained = std::process::Command::new("git")
        .arg("-C")
        .arg(fixture.path()?)
        .args(["gc", "--prune=now"])
        .output()?;
    assert!(
        retained.status.success(),
        "{}",
        String::from_utf8_lossy(&retained.stderr)
    );
    let session = Repository::open(fixture.path()?)?;
    run(&session, BRANCH, ATTEMPT, fixture.captured)?;
    assert_eq!(session.refname_to_id(BRANCH)?, fixture.captured);
    assert_eq!(fs::read(fixture.path()?.join("file"))?, b"record this");
    Ok(())
}

#[cfg(target_os = "linux")]
fn crash_after_index(path: &std::ffi::OsStr) -> Result<()> {
    use std::io::Write as _;
    let repo = Repository::open(Path::new(path))?;
    let journal = Journal::new(&repo, ATTEMPT)?;
    let intent = journal.load(BRANCH)?;
    advance(&repo, &intent, &journal)?;
    advance(&repo, &intent, &journal)?;
    writeln!(std::io::stdout(), "INDEX_INSTALLED")?;
    loop {
        std::thread::park();
    }
}

#[test]
fn blocked_index_lock_preserves_the_other_writers_file_and_resumes_after_release()
-> Result<()> {
    let fixture = Fixture::new()?;
    fixture.start()?;
    let lock = fixture.session.path().join("index.lock");
    fs::write(&lock, "owned by an external writer")?;
    assert!(
        matches!(run(&fixture.session, BRANCH, ATTEMPT, fixture.captured), Err(vcs::Error::Reconciliation { attempt, .. }) if attempt == ATTEMPT)
    );
    assert_eq!(fs::read(&lock)?, b"owned by an external writer");
    assert_eq!(fixture.session.refname_to_id(BRANCH)?, fixture.base);
    fs::remove_file(lock)?;
    run(&fixture.session, BRANCH, ATTEMPT, fixture.captured)?;
    assert_eq!(fixture.session.refname_to_id(BRANCH)?, fixture.captured);
    Ok(())
}

#[test]
fn corrupted_intent_blocks_recovery_without_releasing_or_replacing_work()
-> Result<()> {
    let fixture = Fixture::new()?;
    let journal = fixture.start()?;
    let before = fs::read(fixture.session.path().join("index"))?;
    let path = journal.directory.join("intent");
    let mut bytes = fs::read(&path)?;
    bytes[0] ^= 1;
    fs::write(path, bytes)?;
    assert!(
        matches!(run(&fixture.session, BRANCH, ATTEMPT, fixture.captured), Err(vcs::Error::Reconciliation { attempt, .. }) if attempt == ATTEMPT)
    );
    assert!(ensure_available(&fixture.session, Some("another turn")).is_err());
    assert_eq!(fs::read(fixture.session.path().join("index"))?, before);
    assert_eq!(fixture.session.refname_to_id(BRANCH)?, fixture.base);
    Ok(())
}
