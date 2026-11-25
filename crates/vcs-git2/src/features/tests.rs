use anyhow::Result;
use git2::{Oid, Repository, Signature};
use std::{fs, path::Path};

fn revision(repo: &Repository, paths: &[&str]) -> Result<Oid> {
    let signature = Signature::now("tester", "tester@example.test")?;
    let mut index = repo.index()?;
    for path in paths {
        index.add_path(Path::new(path))?;
    }
    index.write()?;
    let tree = repo.find_tree(index.write_tree()?)?;
    Ok(repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        "fixture",
        &tree,
        &[],
    )?)
}

#[test]
fn only_active_filters_refuse_capture() -> Result<()> {
    let root = tempfile::tempdir()?;
    let repo = Repository::init(root.path())?;
    repo.config()?.set_bool("filter.lfs.required", true)?;
    fs::write(root.path().join("tracked.bin"), "data")?;
    let tip = revision(&repo, &["tracked.bin"])?;
    super::check(&repo, tip)?;
    fs::write(root.path().join(".gitattributes"), "*.bin filter=lfs\n")?;
    let reopened = Repository::open(root.path())?;
    assert!(super::check(&reopened, tip).is_err());
    Ok(())
}

#[test]
fn checkout_preflight_checks_attributes_in_the_requested_revision() -> Result<()>
{
    let root = tempfile::tempdir()?;
    let repo = Repository::init(root.path())?;
    fs::write(root.path().join(".gitattributes"), "*.txt filter=fixture\n")?;
    fs::write(root.path().join("content.txt"), "content")?;
    let candidate = revision(&repo, &[".gitattributes", "content.txt"])?;
    fs::write(root.path().join(".gitattributes"), "*.txt text eol=lf\n")?;
    let mut index = repo.index()?;
    index.add_path(Path::new(".gitattributes"))?;
    index.write()?;
    let reopened = Repository::open(root.path())?;
    let before = fs::read(repo.path().join("index"))?;
    assert!(super::check(&reopened, candidate).is_err());
    assert_eq!(fs::read(repo.path().join("index"))?, before);
    Ok(())
}

#[test]
fn an_index_that_hides_worktree_edits_is_left_untouched() -> Result<()> {
    let root = tempfile::tempdir()?;
    let repo = Repository::init(root.path())?;
    fs::write(root.path().join("local.txt"), "base")?;
    let tip = revision(&repo, &["local.txt"])?;
    let mut index = repo.index()?;
    let mut entry = index
        .get_path(Path::new("local.txt"), 0)
        .ok_or_else(|| anyhow::anyhow!("expected a tracked file"))?;
    entry.flags |= git2::IndexEntryFlag::VALID.bits();
    index.add(&entry)?;
    index.write()?;
    fs::write(root.path().join("local.txt"), "owner's edits")?;
    let before = fs::read(repo.path().join("index"))?;
    assert!(super::check(&repo, tip).is_err());
    assert_eq!(fs::read(repo.path().join("index"))?, before);
    assert_eq!(
        fs::read_to_string(root.path().join("local.txt"))?,
        "owner's edits"
    );
    Ok(())
}

#[test]
fn a_partial_or_shared_object_database_cannot_retain_workspace_history()
-> Result<()> {
    let root = tempfile::tempdir()?;
    let repo = Repository::init(root.path())?;
    fs::write(root.path().join("content"), "base")?;
    let tip = revision(&repo, &["content"])?;
    let info = repo.commondir().join("objects/info");
    fs::create_dir_all(&info)?;
    fs::write(info.join("alternates"), b"/nonexistent-fixture\n")?;
    assert!(super::check(&repo, tip).is_err());
    fs::remove_file(info.join("alternates"))?;
    repo.config()?.set_bool("remote.origin.promisor", true)?;
    assert!(super::check(&repo, tip).is_err());
    Ok(())
}
