//! `home`: the home's command line. `home check` checks every project's
//! issues against the flow (`home::flow::check`), from anywhere in the home.

use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use home::flow::check::{Inputs, check};
use home::flow::graph::File;
use proto::ProjectId;

const FLOW: &str = "docs/agents/flow.toml";
const USAGE: &str = "usage: home check\n";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.as_slice() {
        [command] if command == "check" => run_check(),
        _ => say(USAGE, ExitCode::from(2)),
    }
}

fn run_check() -> ExitCode {
    let Some(root) = root() else {
        return say(
            "home: no home here: no folder above holds docs/agents/flow.toml\n",
            ExitCode::from(2),
        );
    };
    let report = check(&inputs(&root));
    let code = if report.failed() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    };
    say(&report.text(), code)
}

/// The nearest folder at or above this one that holds the flow.
fn root() -> Option<PathBuf> {
    env::current_dir()
        .ok()?
        .ancestors()
        .find(|folder| folder.join(FLOW).is_file())
        .map(Path::to_path_buf)
}

fn inputs(root: &Path) -> Inputs {
    let folders = entries(&root.join("projects"));
    Inputs {
        flow: read(&root.join(FLOW)),
        projects: folders
            .iter()
            .map(|(folder, path)| {
                (folder.clone(), read(&path.join("project.toml")))
            })
            .collect(),
        issues: folders
            .iter()
            .flat_map(|(folder, path)| issues(folder, path))
            .collect(),
    }
}

fn issues(folder: &str, path: &Path) -> Vec<File> {
    entries(&path.join("issues"))
        .into_iter()
        .filter_map(|(name, file)| {
            Some(File {
                project: ProjectId(folder.to_owned()),
                stem: name.strip_suffix(".md")?.to_owned(),
                text: fs::read_to_string(file).ok()?,
            })
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

fn say(text: &str, code: ExitCode) -> ExitCode {
    let _written = std::io::stdout().lock().write_all(text.as_bytes());
    code
}
