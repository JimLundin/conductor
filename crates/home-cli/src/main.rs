//! `home`: the home's command line (`home::flow::command`), run from
//! anywhere in the home. It reads the home's files, the clock and some
//! randomness, and writes the issues the command returns.

use std::env;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use home::flow::PATH;
use home::flow::command::{Context, Write, command, fresh, homeless, run};
use home::flow::graph::File;
use home::flow::snapshot::{Inputs, Snapshot};
use proto::ProjectId;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let root = root();
    let (text, code) = root.as_deref().map_or_else(homeless, |home| {
        let snapshot = Snapshot::of(inputs(home));
        let asked = command(&args, &snapshot);
        let file = asked.file().map(|path| read(Path::new(path)));
        let outcome = run(asked, file, &snapshot, &context());
        let saved = outcome
            .writes
            .iter()
            .try_for_each(|write| save(home, write));
        outcome.ended(saved)
    });
    let _written = std::io::stdout().lock().write_all(text.as_bytes());
    ExitCode::from(code)
}

/// The nearest folder at or above this one that holds the flow.
fn root() -> Option<PathBuf> {
    env::current_dir()
        .ok()?
        .ancestors()
        .find(|folder| folder.join(PATH).is_file())
        .map(Path::to_path_buf)
}

/// Now, the id a new issue takes, and who signs the log: `HOME_BY`, else
/// this host's name and `cli`.
fn context() -> Context {
    let mut random = [0_u8; 16];
    let _filled = getrandom::fill(&mut random);
    let host = env::var("COMPUTERNAME")
        .or_else(|_| env::var("HOSTNAME"))
        .unwrap_or_else(|_| String::from("host"))
        .to_lowercase();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        });
    Context {
        now,
        id: fresh(now, u128::from_le_bytes(random)),
        by: env::var("HOME_BY").unwrap_or_else(|_| format!("{host}·cli")),
    }
}

fn inputs(root: &Path) -> Inputs {
    let folders = entries(&root.join("projects"));
    let (issues, unread): (Vec<_>, Vec<_>) = folders
        .iter()
        .flat_map(|(folder, path)| issues(folder, path))
        .partition(Result::is_ok);
    Inputs {
        flow: fs::read_to_string(root.join(PATH))
            .map_err(|err| err.to_string()),
        projects: folders
            .iter()
            .map(|(folder, path)| {
                (folder.clone(), read(&path.join("project.toml")))
            })
            .collect(),
        issues: issues.into_iter().filter_map(Result::ok).collect(),
        unread: unread.into_iter().filter_map(Result::err).collect(),
    }
}

/// Each `.md` file of a project's issues, or why it couldn't be read.
fn issues(folder: &str, path: &Path) -> Vec<Result<File, String>> {
    entries(&path.join("issues"))
        .into_iter()
        .filter_map(|(name, file)| {
            let stem = name.strip_suffix(".md")?.to_owned();
            Some(read(&file).map(|text| File {
                project: ProjectId(folder.to_owned()),
                stem,
                text,
            }))
        })
        .collect()
}

/// Each entry of `folder` by name, sorted; none when it can't be read.
fn entries(folder: &Path) -> Vec<(String, PathBuf)> {
    let mut found: Vec<(String, PathBuf)> = fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            Some((entry.file_name().into_string().ok()?, entry.path()))
        })
        .collect();
    found.sort();
    found
}

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))
}

fn save(root: &Path, write: &Write) -> Result<(), String> {
    let path = root.join(write.path());
    fs::write(&path, &write.text)
        .map_err(|err| format!("{}: {err}", path.display()))
}
