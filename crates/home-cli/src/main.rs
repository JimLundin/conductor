//! `home`: the home's command line (`home::flow::command`), run from
//! anywhere in the home. It reads the home's files, the clock and some
//! randomness, and writes the issues the command returns; `home repos` and
//! `home skills` are in [`local`].

use std::env;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use home::flow::PATH;
use home::flow::command::{
    Asked, Command, Context, Write, asked, fresh, homeless, run,
};
use home::flow::snapshot::Snapshot;
use proto::Draws;

mod local;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let root = root();
    let (text, code) = root
        .as_deref()
        .map_or_else(homeless, |home| answered(home, &args));
    let _written = std::io::stdout().lock().write_all(text.as_bytes());
    ExitCode::from(code)
}

/// What `args` ask of the home at `home`, done.
fn answered(home: &Path, args: &[String]) -> (String, u8) {
    let snapshot = home_files::read(home);
    let context = context();
    let fresh = fresh(&snapshot, context.now / 1000, &draws());
    match asked(args, &snapshot, fresh) {
        Asked::Local(local) => local::run(home, local, &snapshot),
        Asked::Flow(command) => issues(home, command, &snapshot, &context),
    }
}

/// The flow's `command`, on the home's issues.
fn issues(
    home: &Path,
    asked: Command,
    snapshot: &Snapshot,
    context: &Context,
) -> (String, u8) {
    let file = asked.file().map(|path| read(Path::new(path)));
    let outcome = run(asked, file, snapshot, context);
    let saved = outcome
        .writes
        .iter()
        .try_for_each(|write| save(home, write));
    outcome.ended(saved)
}

/// The nearest folder at or above this one that holds the flow.
fn root() -> Option<PathBuf> {
    env::current_dir()
        .ok()?
        .ancestors()
        .find(|folder| folder.join(PATH).is_file())
        .map(Path::to_path_buf)
}

/// Now, and who signs the log: `HOME_BY`, else this host's name and
/// `cli`.
fn context() -> Context {
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
        by: env::var("HOME_BY").unwrap_or_else(|_| format!("{host}·cli")),
        mark: None,
    }
}

/// The randomness a new issue's or project's id is minted with.
fn draws() -> Draws {
    core::array::from_fn(|_| getrandom::u64().unwrap_or_default())
}

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))
}

fn save(root: &Path, write: &Write) -> Result<(), String> {
    let path = root.join(write.path());
    fs::create_dir_all(root.join(write.folder()))
        .and_then(|()| fs::write(&path, &write.text))
        .map_err(|err| format!("{}: {err}", path.display()))
}
