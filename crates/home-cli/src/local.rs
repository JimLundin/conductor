use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write as _};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};

use home::checkout;
use home::flow::command::Local;
use home::flow::snapshot::Snapshot;
use home::project::Project;
use home::repos::verify::{self, Ran, Verify, Verifying};
use home::repos::{self, New, Placed, Scanning};
use home_files::git;
use vcs::discovery::Probe;
use vcs::status::Checkout;

pub fn run(root: &Path, local: Local, home: &Snapshot) -> (String, u8) {
    match local {
        Local::Status => (status(home, &here(root, home)), 0),
        Local::Verify => {
            let here = here(root, home);
            (status(home, &here) + &verdicts(root, &here), 0)
        }
        Local::Sync => sync(root, home.projects()),
        Local::Scan => ended(scan(root, home)),
        Local::Skills => ended(
            home_files::user()
                .ok_or_else(|| {
                    String::from("no HOME or USERPROFILE to install skills in")
                })
                .and_then(|user| home_files::install(root, &user)),
        ),
    }
}

fn ended(done: Result<String, String>) -> (String, u8) {
    done.map_or_else(|why| (format!("home: {why}\n"), 1), |said| (said, 0))
}

struct Here {
    placed: Vec<Placed>,
    states: BTreeMap<String, Option<Checkout>>,
}

fn here(root: &Path, home: &Snapshot) -> Here {
    let placed = repos::placed(home.projects(), &probes(root));
    let states = repos::checkouts(&placed)
        .into_iter()
        .map(|path| {
            let state = git(
                &root.join(&path),
                &["status", "--porcelain=v2", "--branch"],
            )
            .map(|porcelain| home_files::git::status(&porcelain));
            (path, state)
        })
        .collect();
    Here { placed, states }
}

fn status(home: &Snapshot, here: &Here) -> String {
    repos::warnings(home.listing_problems())
        + &repos::report(&here.placed, &here.states)
}

fn verdicts(root: &Path, here: &Here) -> String {
    let defaults = verify::asks(&here.placed)
        .into_iter()
        .filter_map(|path| {
            let symbolic = git(
                &root.join(&path),
                &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
            )?;
            Some((path, home_files::git::origin_default(&symbolic)?))
        })
        .collect();
    verify::verifying(&shown(root), &here.placed, &here.states, &defaults)
        .into_iter()
        .filter_map(|verifying| match verifying {
            Verifying::Run(verify) => {
                verify::verdict(&verify, &verified(&verify))
            }
            Verifying::Unchecked(reason) => Some(reason),
        })
        .map(|reason| reason.to_string())
        .collect()
}

fn verified(verify: &Verify) -> Ran {
    Command::new("bash")
        .arg(format!("{}/check.sh", verify.profile))
        .args(["commit", "--verify", &verify.checkout])
        .current_dir(&verify.standards)
        .stdin(Stdio::null())
        .output()
        .map_or_else(
            |err| Ran {
                passed: false,
                out: String::new(),
                err: format!("bash: {err}"),
            },
            |output| Ran {
                passed: output.status.success(),
                out: String::from_utf8_lossy(&output.stdout).into_owned(),
                err: String::from_utf8_lossy(&output.stderr).into_owned(),
            },
        )
}

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

fn probes(root: &Path) -> Vec<Probe> {
    let shown = shown(root);
    let paths = checkout::candidates(
        &[home_files::root(Path::new(&shown))],
        std::slice::from_ref(&shown),
    );
    repos::local(&shown, home_files::probes(&paths))
}

fn shown(root: &Path) -> String {
    fs::canonicalize(root)
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .into_owned()
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
