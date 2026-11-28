use std::path::{Path, PathBuf};

use anyhow::Context as _;
use git2::{
    Commit, Index, IndexAddOption, IndexEntry, Oid, Repository, Signature,
};

const EMAIL: &str = "home@conductor.invalid";

pub struct Home {
    repo: Repository,
    prefix: PathBuf,
}

pub fn open(root: &Path) -> Result<Home, vcs::Error> {
    opened(root).map_err(super::failure)
}

fn opened(root: &Path) -> anyhow::Result<Home> {
    let repo = Repository::discover(root)?;
    let workdir = repo
        .workdir()
        .context("the home's repository has no working tree")?
        .canonicalize()?;
    let within = root.canonicalize()?;
    let prefix = within.strip_prefix(&workdir)?.to_path_buf();
    Ok(Home { repo, prefix })
}

impl Home {
    pub fn commit(
        &self,
        paths: &[String],
        author: &str,
        message: &str,
    ) -> Result<Option<String>, vcs::Error> {
        self.recorded(paths, author, message)
            .map_err(super::failure)
    }

    fn recorded(
        &self,
        paths: &[String],
        author: &str,
        message: &str,
    ) -> anyhow::Result<Option<String>> {
        let staged: Vec<PathBuf> =
            paths.iter().map(|path| self.prefix.join(path)).collect();
        let mut index = self.repo.index()?;
        let before: Vec<Option<IndexEntry>> =
            staged.iter().map(|path| index.get_path(path, 0)).collect();
        index.remove_all(&staged, None)?;
        index.add_all(&staged, IndexAddOption::DEFAULT, None)?;
        index.write()?;
        self.signed(&index, &staged, author, message)
            .map_err(|failed| match unstaged(&mut index, &staged, before) {
                Ok(()) => failed,
                Err(undone) => undone.context(format!("{failed:#}")),
            })
    }

    fn signed(
        &self,
        index: &Index,
        staged: &[PathBuf],
        author: &str,
        message: &str,
    ) -> anyhow::Result<Option<String>> {
        let parent = self
            .repo
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
        let written = tree.write_tree_to(&self.repo)?;
        let unchanged = parent
            .as_ref()
            .is_some_and(|head| head.tree_id() == written);
        (!unchanged)
            .then(|| self.made(parent.as_ref(), written, author, message))
            .transpose()
    }

    fn made(
        &self,
        parent: Option<&Commit<'_>>,
        tree: Oid,
        author: &str,
        message: &str,
    ) -> anyhow::Result<String> {
        let signature = Signature::now(author, EMAIL)?;
        let parents: Vec<&Commit<'_>> = parent.into_iter().collect();
        let oid = self.repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &self.repo.find_tree(tree)?,
            &parents,
        )?;
        Ok(oid.to_string())
    }
}

fn unstaged(
    index: &mut Index,
    staged: &[PathBuf],
    before: Vec<Option<IndexEntry>>,
) -> anyhow::Result<()> {
    staged.iter().zip(before).try_for_each(|(path, entry)| {
        index.remove_path(path)?;
        entry.map_or(Ok(()), |kept| index.add(&kept))
    })?;
    index.write()?;
    Ok(())
}
