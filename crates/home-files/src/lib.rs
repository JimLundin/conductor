//! A home read from a folder on disk, as `home::layout` lays it out: the
//! one reading of the home's files that the `home` command line and the
//! host's clone share.

use std::fs;
use std::path::Path;

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
}
