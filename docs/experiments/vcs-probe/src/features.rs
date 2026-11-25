use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
    sync::mpsc,
    thread,
};

use anyhow::{Context, Result, ensure};
use git2::Repository;
use vcs_probe_core::installation::{Cancellation, Effect, cancelled};

use crate::{fixture, snapshot};

fn attributes(root: &Path) -> Result<()> {
    let path = root.join("attributes");
    let (repo, _) = fixture::init(&path)?;
    fixture::write(
        &repo,
        ".gitattributes",
        "* text=auto eol=lf\n*.png binary\n",
    )?;
    fixture::write(&repo, "crlf.txt", "one\r\ntwo\r\n")?;
    fs::write(path.join("image.png"), b"image\r\n")?;
    fs::write(path.join("detected.bin"), b"binary\r\n\0\xff")?;
    fs::set_permissions(
        path.join("crlf.txt"),
        fs::Permissions::from_mode(0o755),
    )?;
    symlink("crlf.txt", path.join("link"))?;
    let candidate = snapshot::capture(&path)?;
    let tree = repo.find_commit(candidate)?.tree_id();
    ensure!(
        fixture::content(&repo, tree, "crlf.txt")? == b"one\ntwo\n",
        "text/eol attribute did not normalize CRLF"
    );
    ensure!(
        fixture::content(&repo, tree, "image.png")? == b"image\r\n",
        "binary attribute changed bytes"
    );
    ensure!(
        fixture::content(&repo, tree, "detected.bin")? == b"binary\r\n\0\xff",
        "text=auto changed binary bytes"
    );
    ensure!(
        fs::read(path.join("crlf.txt"))? == b"one\r\ntwo\r\n",
        "clean filter rewrote source"
    );
    ensure!(
        repo.find_tree(tree)?
            .get_name("crlf.txt")
            .context("missing executable")?
            .filemode()
            == 0o100_755,
        "capture lost executable mode"
    );
    ensure!(
        repo.find_tree(tree)?
            .get_name("link")
            .context("missing symlink")?
            .filemode()
            == 0o120_000,
        "capture lost symlink mode"
    );
    ensure!(
        fixture::content(&repo, tree, "link")? == b"crlf.txt",
        "capture followed symlink"
    );
    let reference = repo.reference(
        "refs/heads/attributes",
        candidate,
        false,
        "attributes",
    )?;
    let checkout = root.join("attributes-checkout");
    repo.worktree(
        "attributes",
        &checkout,
        Some(git2::WorktreeAddOptions::new().reference(Some(&reference))),
    )?;
    ensure!(
        fs::read(checkout.join("crlf.txt"))? == b"one\ntwo\n",
        "checkout did not use LF"
    );
    ensure!(
        fs::read(checkout.join("image.png"))? == b"image\r\n",
        "checkout changed binary bytes"
    );
    ensure!(
        fs::metadata(checkout.join("crlf.txt"))?
            .permissions()
            .mode()
            & 0o111
            != 0,
        "checkout lost executable mode"
    );
    ensure!(
        fs::read_link(checkout.join("link"))? == Path::new("crlf.txt"),
        "checkout followed symlink"
    );
    Ok(())
}

fn custom_filter_is_not_native_git(root: &Path) -> Result<()> {
    let path = root.join("custom-filter");
    let (repo, _) = fixture::init(&path)?;
    let mut config = repo.config()?;
    config.set_str("filter.probe.clean", "this-program-must-never-run")?;
    config.set_str("filter.probe.smudge", "this-program-must-never-run")?;
    config.set_bool("filter.probe.required", true)?;
    fixture::write(&repo, ".gitattributes", "*.txt filter=probe\n")?;
    fixture::write(&repo, "tracked.txt", "raw content\n")?;
    let candidate = snapshot::capture(&path)?;
    ensure!(
        fixture::content(
            &repo,
            repo.find_commit(candidate)?.tree_id(),
            "tracked.txt"
        )? == b"raw content\n",
        "unexpected custom filter result"
    );
    Ok(())
}

fn binary_merge(root: &Path) -> Result<()> {
    let (repo, base) = fixture::init(&root.join("binary-merge"))?;
    fixture::write(&repo, ".gitattributes", "*.png binary\n")?;
    let ancestor = fixture::commit(
        &repo,
        Some(base),
        &[("image.png", "first\nsecond\nthird\n")],
        "binary ancestor",
    )?;
    let left = fixture::commit(
        &repo,
        Some(ancestor),
        &[("image.png", "left\nsecond\nthird\n")],
        "binary left",
    )?;
    let right = fixture::commit(
        &repo,
        Some(ancestor),
        &[("image.png", "first\nsecond\nright\n")],
        "binary right",
    )?;
    let merged = repo.merge_commits(
        &repo.find_commit(left)?,
        &repo.find_commit(right)?,
        None,
    )?;
    ensure!(
        merged.has_conflicts(),
        "binary attribute allowed a textual merge"
    );
    ensure!(
        repo.head()?.target() == Some(base),
        "binary merge changed HEAD"
    );
    Ok(())
}

fn blocking_cancellation(root: &Path) -> Result<()> {
    let path = root.join("cancellation");
    let (repo, base) = fixture::init(&path)?;
    let candidate = fixture::commit(
        &repo,
        Some(base),
        &[("later.txt", "later\n")],
        "later",
    )?;
    repo.reference("refs/heads/cancel", base, false, "before dispatch")?;
    let (entered, running) = mpsc::sync_channel(0);
    let (release, continue_effect) = mpsc::sync_channel(0);
    let (finished, completion) = mpsc::sync_channel(0);
    let worker = thread::spawn(move || -> Result<()> {
        let opened = Repository::open(path)?;
        entered.send(())?;
        continue_effect.recv()?;
        opened.reference_matching(
            "refs/heads/cancel",
            candidate,
            true,
            base,
            "in-flight effect",
        )?;
        finished.send(())?;
        Ok(())
    });
    running.recv()?;
    ensure!(
        cancelled(Cancellation::BeforeDispatch) == Effect::NotStarted,
        "pre-dispatch cancellation has effects"
    );
    ensure!(
        cancelled(Cancellation::Running) == Effect::InFlight,
        "running effect falsely cancelled"
    );
    drop(worker);
    release.send(())?;
    completion.recv()?;
    ensure!(
        repo.refname_to_id("refs/heads/cancel")? == candidate,
        "detached worker did not finish"
    );
    ensure!(
        cancelled(Cancellation::ReturnedApplied) == Effect::Applied,
        "confirmed effect lost to cancellation"
    );
    ensure!(
        cancelled(Cancellation::ReturnedUncertain) == Effect::Unknown,
        "unobserved effect falsely succeeded"
    );
    Ok(())
}

fn stale_ref_lock(root: &Path) -> Result<()> {
    let (repo, base) = fixture::init(&root.join("stale-ref-lock"))?;
    let path = repo.path().join("refs/heads/main.lock");
    fs::write(&path, "unknown owner")?;
    let result = repo.reference_matching(
        "refs/heads/main",
        base,
        true,
        base,
        "must refuse",
    );
    ensure!(
        result.is_err_and(|error| error.code() == git2::ErrorCode::Locked),
        "stale ref lock bypassed"
    );
    ensure!(
        fs::read(&path)? == b"unknown owner",
        "stale ref lock deleted"
    );
    ensure!(
        repo.head()?.target().context("missing HEAD")? == base,
        "stale lock changed HEAD"
    );
    Ok(())
}

pub fn run(root: &Path) -> Result<()> {
    attributes(root)?;
    custom_filter_is_not_native_git(root)?;
    binary_merge(root)?;
    blocking_cancellation(root)?;
    stale_ref_lock(root)?;
    Ok(())
}
