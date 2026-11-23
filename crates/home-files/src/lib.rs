//! A home read from a folder on disk, as `home::layout` lays it out, and
//! the repositories on this machine as Git describes them: the one reading
//! of each that the `home` command line and the host share.

use std::fs;
use std::path::Path;
use std::process::Command;

use home::checkout::{self, Probe, Root};
use home::flow::PATH;
use home::flow::snapshot::Snapshot;
use home::layout::{self, IssueLocation};
use proto::ProjectId;

/// The home whose root is `root`.
#[must_use]
pub fn read(root: &Path) -> Snapshot {
    let folders = folders(&root.join(layout::PROJECTS));
    Snapshot::of(layout::inputs(
        text(root, PATH),
        folders
            .iter()
            .map(|folder| {
                (folder.clone(), text(root, &layout::project_file(folder)))
            })
            .collect(),
        folders
            .iter()
            .filter_map(|folder| ProjectId::parse(folder))
            .flat_map(|project| issues(root, project))
            .collect(),
    ))
}

/// A search root with the folders in it and in each of those that isn't
/// hidden, so a checkout one level down is found too.
#[must_use]
pub fn root(path: &Path) -> Root {
    let mut entries: Vec<String> = folders(path)
        .into_iter()
        .flat_map(|name| {
            let deeper = if name.starts_with('.') {
                Vec::new()
            } else {
                folders(&path.join(&name))
            };
            let inner: Vec<String> = deeper
                .into_iter()
                .map(|inner| format!("{name}/{inner}"))
                .collect();
            core::iter::once(name).chain(inner)
        })
        .collect();
    entries.sort();
    Root {
        path: path.to_string_lossy().into_owned(),
        entries,
    }
}

/// The repository at each of `paths` that is the top of one, as
/// [`checkout::probed`] reads it.
#[must_use]
pub fn probes(paths: &[String]) -> Vec<Probe> {
    paths
        .iter()
        .filter_map(|path| probe(Path::new(path)))
        .collect()
}

/// What Git says of the repository at `path`, by its canonical path. A
/// folder without `.git` is the top of none, so Git isn't asked there.
fn probe(path: &Path) -> Option<Probe> {
    let at = fs::canonicalize(path)
        .ok()
        .filter(|at| at.join(".git").exists())?;
    let asked = |args: &[&str]| git(&at, args);
    checkout::probed(
        at.to_str()?,
        asked(&["rev-parse", "--show-toplevel"]).as_deref(),
        asked(&["rev-list", "--max-parents=0", "HEAD"]).as_deref(),
        asked(&["remote", "get-url", "origin"]).as_deref(),
    )
}

/// What Git printed in `at`, if it succeeded, with no `GIT_` variable of
/// this process's to point it elsewhere.
#[must_use]
pub fn git(at: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(at)
        .args(["-c", "core.fsmonitor=false"])
        .args(args);
    std::env::vars_os()
        .map(|(key, _)| key)
        .filter(|key| key.to_string_lossy().starts_with("GIT_"))
        .for_each(|key| {
            command.env_remove(key);
        });
    command
        .output()
        .ok()
        .filter(|ran| ran.status.success())
        .and_then(|ran| String::from_utf8(ran.stdout).ok())
}

/// The names in the folder at `path`, none when it can't be read.
#[must_use]
pub fn names(path: &Path) -> Vec<String> {
    entries(path).map(|(name, _)| name).collect()
}

/// Each issue file of `project`, with its text or why it couldn't be read.
fn issues(
    root: &Path,
    project: ProjectId,
) -> Vec<(IssueLocation, Result<String, String>)> {
    let names = entries(&root.join(layout::issues(&project)))
        .map(|(name, _)| name)
        .collect();
    layout::issue_files(&project, names)
        .into_iter()
        .map(|file| {
            let read = text(root, &file.path);
            (file, read)
        })
        .collect()
}

/// The names of the folders in `path`, sorted.
fn folders(path: &Path) -> Vec<String> {
    let mut found: Vec<String> = entries(path)
        .filter(|(_, folder)| *folder)
        .map(|(name, _)| name)
        .collect();
    found.sort();
    found
}

/// Each entry of `path` by name, and whether it is a folder; none when it
/// can't be read.
fn entries(path: &Path) -> impl Iterator<Item = (String, bool)> {
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let folder = entry.path().is_dir();
            Some((entry.file_name().into_string().ok()?, folder))
        })
}

/// The text of the file at `path` from `root`, or why it couldn't be read.
fn text(root: &Path, path: &str) -> Result<String, String> {
    fs::read_to_string(root.join(path)).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _removed = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_home_s_md_issue_files_are_read_beside_its_projects()
    -> std::io::Result<()> {
        let scratch = Scratch(
            std::env::temp_dir()
                .join(format!("home-files-{}", std::process::id())),
        );
        let issues = scratch.0.join("projects/a1b2c3/issues");
        fs::create_dir_all(&issues)?;
        fs::write(issues.join("one.md"), "+++\n+++\n")?;
        fs::write(issues.join("notes.txt"), "not an issue")?;
        let home = super::read(&scratch.0);
        let stems: Vec<&str> =
            home.files().iter().map(|file| file.stem.as_str()).collect();
        assert_eq!(stems, ["one"]);
        assert!(
            home.listing_problems()
                .iter()
                .any(|problem| problem.starts_with("projects/a1b2c3: ")),
            "{:?}",
            home.listing_problems()
        );
        Ok(())
    }

    fn committed(at: &std::path::Path) -> std::io::Result<()> {
        fs::create_dir_all(at.join("inner"))?;
        fs::write(at.join("README.md"), "")?;
        [
            &["init", "-q"][..],
            &["add", "."],
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "first",
            ],
        ]
        .iter()
        .try_for_each(|args| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(at)
                .args(*args)
                .status()
                .map(drop)
        })
    }

    #[test]
    fn only_the_top_of_a_repository_outside_a_set_aside_folder_is_found()
    -> std::io::Result<()> {
        let scratch = Scratch(
            std::env::temp_dir()
                .join(format!("home-files-probes-{}", std::process::id())),
        );
        committed(&scratch.0.join("apps/one"))?;
        committed(&scratch.0.join("_old/two"))?;
        let at = fs::canonicalize(&scratch.0)?;
        let paths = home::checkout::candidates(&[super::root(&at)], &[]);
        let found: Vec<String> = super::probes(&paths)
            .into_iter()
            .map(|probe| probe.path)
            .collect();
        assert_eq!(found, [at.join("apps/one").display().to_string()]);
        Ok(())
    }
}
