use std::env;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use home::Draws;
use home::flow::PATH;
use home::flow::command::{
    Asked, Command, Context, asked, fresh, homeless, run,
};
use home::issue::storage::File;

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

fn answered(home: &Path, args: &[String]) -> (String, u8) {
    let loaded = home_files::read(home);
    let snapshot = &loaded.snapshot;
    let context = context();
    let fresh = fresh(snapshot, context.now / 1000, &draws());
    match asked(args, snapshot, fresh) {
        Asked::Local(local) => local::run(home, local, snapshot),
        Asked::Flow(command) => issues(home, command, &loaded, &context),
    }
}

fn issues(
    home: &Path,
    asked: Command,
    loaded: &home::issue::storage::ReadHome,
    context: &Context,
) -> (String, u8) {
    let file = asked.file().map(|path| read(Path::new(path)));
    let outcome = run(asked, file, &loaded.snapshot, context);
    let saved = outcome.changes.iter().try_for_each(|change| {
        loaded.encode(change).and_then(|file| save(home, &file))
    });
    outcome.ended(saved)
}

fn root() -> Option<PathBuf> {
    env::current_dir()
        .ok()?
        .ancestors()
        .find(|folder| folder.join(PATH).is_file())
        .map(Path::to_path_buf)
}

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
    }
}

fn draws() -> Draws {
    core::array::from_fn(|_| getrandom::u64().unwrap_or_default())
}

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))
}

fn save(root: &Path, write: &File) -> Result<(), String> {
    let path = root.join(write.path());
    fs::create_dir_all(root.join(write.folder()))
        .and_then(|()| fs::write(&path, &write.text))
        .map_err(|err| format!("{}: {err}", path.display()))
}
