use std::path::{Path, PathBuf};

use anyhow::Context as _;
use git2::{Commit, Index, IndexAddOption, Oid, Repository, Signature};

pub fn commit(
    root: &Path,
    paths: &[String],
    author: &str,
    message: &str,
) -> Result<Option<String>, vcs::Error> {
    recorded(root, paths, author, message).map_err(super::failure)
}

fn recorded(
    root: &Path,
    paths: &[String],
    author: &str,
    message: &str,
) -> anyhow::Result<Option<String>> {
    let repo = Repository::discover(root)?;
    let staged = staged(&repo, root, paths)?;
    let mut index = repo.index()?;
    index.remove_all(&staged, None)?;
    index.add_all(&staged, IndexAddOption::DEFAULT, None)?;
    index.write()?;
    let parent = repo
        .head()
        .ok()
        .map(|head| head.peel_to_commit())
        .transpose()?;
    let mut tree = Index::new()?;
    parent
        .as_ref()
        .map(|head| head.tree().and_then(|files| tree.read_tree(&files)))
        .transpose()?;
    staged.iter().try_for_each(|path| {
        tree.remove_path(path)?;
        index
            .get_path(path, 0)
            .map_or(Ok(()), |entry| tree.add(&entry))
    })?;
    let written = tree.write_tree_to(&repo)?;
    let unchanged = parent
        .as_ref()
        .is_some_and(|head| head.tree_id() == written);
    (!unchanged)
        .then(|| signed(&repo, parent.as_ref(), written, author, message))
        .transpose()
}

fn staged(
    repo: &Repository,
    root: &Path,
    paths: &[String],
) -> anyhow::Result<Vec<PathBuf>> {
    let workdir = repo
        .workdir()
        .context("the home's repository has no working tree")?
        .canonicalize()?;
    let within = root.canonicalize()?;
    let prefix = within.strip_prefix(&workdir)?;
    Ok(paths.iter().map(|path| prefix.join(path)).collect())
}

fn signed(
    repo: &Repository,
    parent: Option<&Commit<'_>>,
    tree: Oid,
    author: &str,
    message: &str,
) -> anyhow::Result<String> {
    let email = repo
        .config()
        .and_then(|config| config.get_string("user.email"))
        .unwrap_or_default();
    let signature = Signature::now(author, &email)?;
    let parents: Vec<&Commit<'_>> = parent.into_iter().collect();
    let oid = repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        message,
        &repo.find_tree(tree)?,
        &parents,
    )?;
    Ok(oid.to_string())
}
