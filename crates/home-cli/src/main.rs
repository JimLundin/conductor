use std::env;
use std::fs;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use home::Draws;
use home::canvas::{CanvasFile, CanvasKey, CanvasName, Held};
use home::flow::PATH;
use home::flow::command::{
    Asked, Command, Context, Given, Need, Source, Stored, Wanted, Write, asked,
    fresh, homeless, run,
};
use home::flow::snapshot::Snapshot;
use home_files::FolderStore;
use home_store::{CanvasStore, StoreError};

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
    let snapshot = home_files::read(home);
    let mut store = FolderStore::new(home.to_path_buf());
    let context = context();
    let fresh = fresh(&snapshot, context.now / 1000, &draws());
    match asked(args, &snapshot, fresh) {
        Asked::Local(local) => local::run(home, local, &snapshot),
        Asked::Flow(command) => {
            issues(home, &mut store, command, &snapshot, &context)
        }
    }
}

fn issues(
    home: &Path,
    store: &mut dyn CanvasStore,
    asked: Command,
    snapshot: &Snapshot,
    context: &Context,
) -> (String, u8) {
    let given = asked.needs().map(|need| match need {
        Need::Read(Source::File(path)) => Given::Text(read(Path::new(&path))),
        Need::Read(Source::Stdin) => Given::Text(stdin()),
        Need::Canvases(wanted) => Given::Canvases(fetched(&*store, &wanted)),
        Need::EveryCanvas => Given::EveryCanvas(every(&*store)),
    });
    let outcome = run(asked, given, snapshot, context);
    let saved = outcome
        .writes
        .iter()
        .try_for_each(|write| save(home, write))
        .and_then(|()| {
            outcome.canvases.iter().try_for_each(|canvas| {
                store
                    .put(&canvas.key, &canvas.file)
                    .map_err(|error| error.to_string())
            })
        });
    outcome.ended(saved)
}

fn fetched(
    store: &dyn CanvasStore,
    wanted: &Wanted,
) -> Result<Held<(CanvasName, CanvasFile)>, String> {
    let (names, strays) = if let Some(name) = &wanted.name {
        (Vec::from([name.clone()]), Vec::new())
    } else {
        let held = store
            .list(&wanted.project, &wanted.issue)
            .map_err(|error| error.to_string())?;
        (
            held.canvases.into_iter().map(|entry| entry.name).collect(),
            held.strays,
        )
    };
    let canvases = names
        .into_iter()
        .filter_map(|name| {
            let key = CanvasKey {
                project: wanted.project,
                issue: wanted.issue,
                name,
            };
            store
                .get(&key)
                .map(|kept| kept.map(|file| (key.name, file)))
                .map_err(|error| error.to_string())
                .transpose()
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Held { canvases, strays })
}

fn every(store: &dyn CanvasStore) -> Result<Held<Stored>, String> {
    let held = store.all().map_err(|error| error.to_string())?;
    Ok(Held {
        canvases: held
            .canvases
            .into_iter()
            .map(|key| {
                let got = store.get(&key).map_err(|refused| match refused {
                    StoreError::Invalid { error, .. } => error.to_string(),
                    StoreError::Unavailable { reason } => reason,
                });
                Stored { key, got }
            })
            .collect(),
        strays: held.strays,
    })
}

fn stdin() -> Result<String, String> {
    let mut text = String::new();
    io::stdin()
        .read_to_string(&mut text)
        .map(|_| text)
        .map_err(|err| format!("stdin: {err}"))
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
        mark: None,
    }
}

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
