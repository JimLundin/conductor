//! `home repos` and `home skills`: the home's projects among the
//! repositories on this machine, and its skills in each harness's folder.
//! It lists folders, runs Git, and writes and copies files.

use std::env;
use std::fs;
use std::io::{self, Write as _};
use std::path::Path;
use std::process::{Command, ExitStatus};

use home::checkout::{self, Probe};
use home::flow::command::Local;
use home::flow::snapshot::Snapshot;
use home::project::Project;
use home::repos::{self, New, Scanning};
use home::skills::{self, HARNESSES, LISTING, SKILL_FILE, SKILLS};
use home_files::git;

/// What doing `local` in the home at `root` says, and its exit code.
pub fn run(root: &Path, local: Local, home: &Snapshot) -> (String, u8) {
    match local {
        Local::Status => (status(root, home), 0),
        Local::Sync => sync(root, home.projects()),
        Local::Scan => ended(scan(root, home)),
        Local::Skills => ended(copied(root)),
    }
}

fn ended(done: Result<String, String>) -> (String, u8) {
    done.map_or_else(|why| (format!("home: {why}\n"), 1), |said| (said, 0))
}

fn status(root: &Path, home: &Snapshot) -> String {
    let placed = repos::placed(home.projects(), &probes(root));
    let states = repos::checkouts(&placed)
        .into_iter()
        .map(|path| {
            let state =
                git(&root.join(&path), &["status", "--porcelain", "--branch"]);
            (path, state)
        })
        .collect();
    repos::warnings(home.listing_problems()) + &repos::report(&placed, &states)
}

/// Clones each project with no checkout here, saying each as it goes, so
/// Git's own output follows it.
fn sync(root: &Path, projects: &[Project]) -> (String, u8) {
    let failures: Vec<String> =
        repos::sync(&repos::placed(projects, &probes(root)))
            .into_iter()
            .filter_map(|fetch| {
                let _said =
                    io::stdout().lock().write_all(fetch.said.as_bytes());
                fetch.clone.and_then(|(remote, into)| {
                    clone(root, &remote, &into).err()
                })
            })
            .collect();
    (failures.concat(), u8::from(!failures.is_empty()))
}

fn clone(root: &Path, remote: &str, into: &str) -> Result<(), String> {
    Command::new("git")
        .arg("clone")
        .arg(remote)
        .arg(root.join(into))
        .status()
        .ok()
        .filter(ExitStatus::success)
        .map(drop)
        .ok_or_else(|| format!("home: cloning {remote} into {into} failed\n"))
}

fn scan(root: &Path, home: &Snapshot) -> Result<String, String> {
    let projects = home.projects();
    let new = repos::unmatched(projects, &probes(root))
        .into_iter()
        .map(|probe| New {
            names: home_files::names(&root.join(&probe.path)),
            draws: crate::draws(),
            probe,
        })
        .collect();
    let done = repos::scan(Scanning {
        home: root.file_name().map_or_else(String::new, |name| {
            name.to_string_lossy().into_owned()
        }),
        projects: projects.to_vec(),
        new,
        problems: home.listing_problems().to_vec(),
        doc: read(&root.join(repos::DOC)),
    })?;
    done.writes
        .iter()
        .try_for_each(|(path, text)| write(&root.join(path), text))?;
    Ok(done.said)
}

/// Each repository in the home, the home among them, found as the host
/// finds checkouts with the home as its one root.
fn probes(root: &Path) -> Vec<Probe> {
    let home = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let shown = home.to_string_lossy().into_owned();
    let paths = checkout::candidates(
        &[home_files::root(&home)],
        std::slice::from_ref(&shown),
    );
    repos::local(&shown, home_files::probes(&paths))
}

/// Copies the home's skills into each harness's folder in the user's home.
fn copied(root: &Path) -> Result<String, String> {
    let user = env::var("HOME")
        .or_else(|_| env::var("USERPROFILE"))
        .map_err(|_| {
            String::from("no HOME or USERPROFILE to copy skills into")
        })?;
    let source = root.join(SKILLS);
    let offered: Vec<(String, bool)> = home_files::names(&source)
        .into_iter()
        .map(|name| {
            let skill = source.join(&name).join(SKILL_FILE).is_file();
            (name, skill)
        })
        .collect();
    HARNESSES
        .iter()
        .map(|harness| {
            harnessed(&format!("{user}/{harness}"), &source, offered.clone())
                .map_err(|err| format!("{user}/{harness}: {err}"))
        })
        .collect()
}

fn harnessed(
    folder: &str,
    source: &Path,
    offered: Vec<(String, bool)>,
) -> io::Result<String> {
    let at = Path::new(folder);
    fs::create_dir_all(at)?;
    let listing = fs::read_to_string(at.join(LISTING)).ok();
    let plan = skills::plan(
        folder,
        listing.as_deref(),
        &home_files::names(at),
        offered,
    );
    plan.remove
        .iter()
        .map(|name| at.join(name))
        .filter(|path| path.exists())
        .try_for_each(fs::remove_dir_all)?;
    fs::write(at.join(LISTING), &plan.listing)?;
    plan.copy
        .iter()
        .try_for_each(|name| copy(&source.join(name), &at.join(name)))?;
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

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|err| err.to_string())
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    path.parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| fs::write(path, text))
        .map_err(|err| format!("{}: {err}", path.display()))
}
