//! `home`: the home's command line (`home::flow::command`), run from
//! anywhere in the home. It reads the home's files, the clock and some
//! randomness, and writes the issues the command returns.

use std::env;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use home::flow::check::Inputs;
use home::flow::command::{Context, Write, command, fresh, run};
use home::flow::graph::File;
use proto::ProjectId;

const FLOW: &str = "docs/agents/flow.toml";

fn main() -> ExitCode {
    let given: Vec<String> = env::args().skip(1).collect();
    let Some(root) = root() else {
        return say(
            "home: no home here: no folder above holds docs/agents/flow.toml\n",
            ExitCode::from(2),
        );
    };
    let args = match with_file(given) {
        Ok(read) => read,
        Err(reason) => {
            return say(&format!("home: {reason}\n"), ExitCode::FAILURE);
        }
    };
    let outcome = run(&command(&args), &inputs(&root), &context());
    let written = outcome
        .writes
        .iter()
        .try_for_each(|write| save(&root, write));
    let text = written.as_ref().map_or_else(
        |err| format!("{}home: {err}\n", outcome.output),
        |()| outcome.output.clone(),
    );
    match (written.is_ok(), outcome.failed) {
        (true, false) => say(&text, ExitCode::SUCCESS),
        _ => say(&text, ExitCode::FAILURE),
    }
}

/// The nearest folder at or above this one that holds the flow.
fn root() -> Option<PathBuf> {
    env::current_dir()
        .ok()?
        .ancestors()
        .find(|folder| folder.join(FLOW).is_file())
        .map(Path::to_path_buf)
}

/// The words, with the text of the file `home issue body <id> --file
/// <path>` names in place of the flag and its path.
fn with_file(args: Vec<String>) -> Result<Vec<String>, String> {
    match args.as_slice() {
        [issue, body, id, flag, path]
            if issue == "issue" && body == "body" && flag == "--file" =>
        {
            let text = read(Path::new(path))?;
            Ok(Vec::from([issue.clone(), body.clone(), id.clone(), text]))
        }
        _ => Ok(args),
    }
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

fn save(root: &Path, write: &Write) -> Result<(), String> {
    let path = root
        .join("projects")
        .join(&write.project.0)
        .join("issues")
        .join(format!("{}.md", write.stem));
    fs::write(&path, &write.text)
        .map_err(|err| format!("{}: {err}", path.display()))
}

fn say(text: &str, code: ExitCode) -> ExitCode {
    let _written = std::io::stdout().lock().write_all(text.as_bytes());
    code
}
