use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use home::ProjectId;
use home::checkout::Root;
use vcs::discovery::Probe;

use home::flow::PATH;
use home::issue::{self, storage};
use home::layout::{self, IssueLocation};
use home::skills;

pub mod git;

#[must_use]
pub fn has_configuration(root: &Path) -> bool {
    folders(&root.join(layout::PROJECTS)).iter().any(|folder| {
        ProjectId::parse(folder).is_some()
            && root.join(layout::project_file(folder)).is_file()
    })
}

#[must_use]
pub fn configuration(root: &Path) -> home::flow::snapshot::Snapshot {
    layout::inputs(text(root, PATH), project_files(root), Vec::new())
        .read()
        .snapshot
}

#[must_use]
pub fn read(root: &Path) -> storage::ReadHome {
    let projects = project_files(root);
    let records = projects
        .iter()
        .filter_map(|(folder, _)| ProjectId::parse(folder))
        .flat_map(|project| issues(root, project))
        .collect();
    let mut inputs = layout::inputs(text(root, PATH), projects, records);
    inputs.observed_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok());
    inputs.read()
}

fn project_files(root: &Path) -> Vec<(String, Result<String, String>)> {
    folders(&root.join(layout::PROJECTS))
        .into_iter()
        .map(|folder| {
            let read = text(root, &layout::project_file(&folder));
            (folder, read)
        })
        .collect()
}

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

#[must_use]
pub fn probes(paths: &[String]) -> Vec<Probe> {
    paths
        .iter()
        .filter_map(|path| probe(Path::new(path)))
        .collect()
}

fn probe(path: &Path) -> Option<Probe> {
    let at = fs::canonicalize(path).ok()?;
    let top = repository_text(&at, &git::Instruction::Root)?;
    let roots = repository_text(&at, &git::Instruction::RootCommits)?;
    let remote =
        repository_text(&at, &git::Instruction::Origin { push: false });
    git::probed(at.to_str()?, Some(&top), Some(&roots), remote.as_deref())
}

fn repository_text(at: &Path, request: &git::Instruction) -> Option<String> {
    let command = git::command(request);
    let args: Vec<&str> = command.args.iter().map(String::as_str).collect();
    git(at, &args)
}

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

#[must_use]
pub fn names(path: &Path) -> Vec<String> {
    entries(path).map(|(name, _)| name).collect()
}

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

fn folders(path: &Path) -> Vec<String> {
    let mut found: Vec<String> = entries(path)
        .filter(|(_, folder)| *folder)
        .map(|(name, _)| name)
        .collect();
    found.sort();
    found
}

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

#[must_use]
pub fn user() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

pub fn install(home: &Path, user: &Path) -> Result<String, String> {
    let source = home.join(skills::SKILLS);
    let offered: Vec<(String, bool)> = folders(&source)
        .into_iter()
        .map(|name| {
            let skill = source.join(&name).join(skills::SKILL_FILE).is_file();
            (name, skill)
        })
        .collect();
    skills::HARNESSES
        .iter()
        .map(|harness| {
            let folder = user.join(harness);
            harnessed(&folder, &source, offered.clone())
                .map_err(|err| format!("{}: {err}", folder.display()))
        })
        .collect()
}

fn harnessed(
    folder: &Path,
    source: &Path,
    offered: Vec<(String, bool)>,
) -> io::Result<String> {
    fs::create_dir_all(folder)?;
    let listing = fs::read_to_string(folder.join(skills::LISTING)).ok();
    let plan = skills::plan(
        &folder.display().to_string(),
        listing.as_deref(),
        &names(folder),
        offered,
    );
    plan.remove
        .iter()
        .map(|name| folder.join(name))
        .filter(|path| path.exists())
        .try_for_each(fs::remove_dir_all)?;
    fs::write(folder.join(skills::LISTING), &plan.listing)?;
    plan.copy
        .iter()
        .try_for_each(|name| copy(&source.join(name), &folder.join(name)))?;
    Ok(plan.said)
}

fn copy(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    fs::read_dir(from)?.try_for_each(|read| {
        let entry = read?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy(&entry.path(), &target)
        } else {
            fs::copy(entry.path(), target).map(drop)
        }
    })
}

fn text(root: &Path, path: &str) -> Result<String, String> {
    fs::read_to_string(root.join(path)).map_err(|err| err.to_string())
}

#[must_use]
pub fn issue(
    flow: &home::flow::Flow,
    target: &issue::Read,
    text: Option<&str>,
) -> Option<storage::Loaded> {
    text.map(|contents| storage::load(flow, target, contents))
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

    fn skill(at: &std::path::Path, text: &str) -> std::io::Result<()> {
        fs::create_dir_all(at)?;
        fs::write(at.join("SKILL.md"), text)
    }

    #[test]
    fn install_replaces_its_own_copies_and_keeps_skills_installed_some_other_way()
    -> Result<(), Box<dyn std::error::Error>> {
        let scratch = Scratch(
            std::env::temp_dir()
                .join(format!("home-files-skills-{}", std::process::id())),
        );
        let (home, user) = (scratch.0.join("home"), scratch.0.join("user"));
        let skills = home.join("skills");
        skill(&skills.join("build"), "build")?;
        skill(&skills.join("tdd"), "tdd")?;
        fs::create_dir_all(skills.join("tdd/more"))?;
        fs::write(skills.join("tdd/more/cycle.md"), "red, green")?;
        skill(&skills.join("theirs"), "the home's")?;
        fs::create_dir_all(skills.join("notes"))?;
        let folders =
            [".claude/skills", ".agents/skills"].map(|at| user.join(at));
        skill(&folders[0].join("theirs"), "installed by hand")?;

        super::install(&home, &user)?;
        for folder in &folders {
            assert_eq!(
                fs::read_to_string(folder.join("build/SKILL.md"))?,
                "build"
            );
            assert_eq!(
                fs::read_to_string(folder.join("tdd/more/cycle.md"))?,
                "red, green"
            );
            assert!(!folder.join("notes").exists());
        }
        assert_eq!(
            fs::read_to_string(folders[0].join("theirs/SKILL.md"))?,
            "installed by hand"
        );
        assert_eq!(
            fs::read_to_string(folders[1].join("theirs/SKILL.md"))?,
            "the home's"
        );

        fs::remove_dir_all(skills.join("tdd"))?;
        fs::write(skills.join("build/SKILL.md"), "build again")?;
        let outside = user.join(".claude/outside");
        fs::create_dir_all(&outside)?;
        let listing = folders[0].join(".home-skills");
        let listed = fs::read_to_string(&listing)?;
        fs::write(&listing, format!("{listed}../outside\n"))?;
        let said = super::install(&home, &user)?;
        assert!(outside.is_dir(), "a listed path out of the folder stays");
        for folder in &folders {
            assert!(!folder.join("tdd").exists(), "{said}");
            assert_eq!(
                fs::read_to_string(folder.join("build/SKILL.md"))?,
                "build again"
            );
        }
        assert_eq!(
            fs::read_to_string(folders[0].join("theirs/SKILL.md"))?,
            "installed by hand"
        );
        assert!(said.contains("theirs"), "{said}");
        Ok(())
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
        let flow = scratch.0.join(home::flow::PATH);
        fs::create_dir_all(flow.parent().unwrap_or(&scratch.0))?;
        fs::write(flow, include_str!("../../home/src/flow/test-flow.toml"))?;
        let home = super::read(&scratch.0).snapshot;
        let stems: Vec<&str> = home
            .graph()
            .problems
            .iter()
            .map(|problem| problem.record.as_str())
            .collect();
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

    #[test]
    fn configuration_reads_do_not_refresh_issue_records() -> std::io::Result<()>
    {
        let scratch = Scratch(
            std::env::temp_dir()
                .join(format!("home-configuration-{}", std::process::id())),
        );
        let project = scratch.0.join("projects/a1b2c3");
        fs::create_dir_all(project.join("issues"))?;
        fs::write(
            project.join("project.toml"),
            "name = \"local\"\ntitle = \"Local\"\nroot = \"fc27e50efb661d7ef4a45d3edbe0234ff0a6fbd5\"\n",
        )?;
        fs::write(project.join("issues/invalid.md"), "not an issue")?;
        let flow = scratch.0.join(home::flow::PATH);
        fs::create_dir_all(flow.parent().unwrap_or(&scratch.0))?;
        fs::write(flow, include_str!("../../home/src/flow/test-flow.toml"))?;
        let configuration = super::configuration(&scratch.0);
        assert_eq!(configuration.projects().len(), 1);
        assert_eq!(configuration.graph().problems, Vec::new());
        assert_eq!(configuration.graph().issues, Vec::new());
        Ok(())
    }

    fn git(
        at: &std::path::Path,
        args: &[&str],
        date: Option<&str>,
    ) -> std::io::Result<String> {
        let mut command = std::process::Command::new("git");
        command
            .arg("-C")
            .arg(at)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args);
        if let Some(when) = date {
            command
                .env("GIT_AUTHOR_DATE", when)
                .env("GIT_COMMITTER_DATE", when);
        }
        let ran = command.output()?;
        Ok(String::from_utf8_lossy(&ran.stdout).trim().to_owned())
    }

    fn committed(at: &std::path::Path) -> std::io::Result<String> {
        fs::create_dir_all(at.join("inner"))?;
        fs::write(at.join("README.md"), "")?;
        git(at, &["init", "-q", "-b", "main"], None)?;
        git(at, &["add", "."], None)?;
        git(at, &["commit", "-qm", "first"], None)?;
        git(at, &["rev-parse", "HEAD"], None)
    }

    #[test]
    fn a_repository_s_root_is_the_first_commit_of_its_first_parent_history()
    -> std::io::Result<()> {
        let scratch = Scratch(
            std::env::temp_dir()
                .join(format!("home-files-roots-{}", std::process::id())),
        );
        let at = &scratch.0;
        let first = committed(at)?;
        git(at, &["checkout", "-q", "--orphan", "older"], None)?;
        fs::write(at.join("older.md"), "")?;
        git(at, &["add", "."], None)?;
        git(
            at,
            &["commit", "-qm", "older"],
            Some("2000-01-01T00:00:00Z"),
        )?;
        git(at, &["checkout", "-q", "main"], None)?;
        git(
            at,
            &[
                "merge",
                "-q",
                "--allow-unrelated-histories",
                "-m",
                "in",
                "older",
            ],
            None,
        )?;
        let found = super::probes(&[at.display().to_string()]);
        let roots: Vec<&[String]> =
            found.iter().map(|probe| probe.roots.as_slice()).collect();
        assert_eq!(roots, [[first]]);
        Ok(())
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
