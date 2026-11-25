use home::issue::storage::fresh;
use std::env;
use std::fs;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use home::canvas::{CanvasKey, Held};
use home::flow::PATH;
use home::flow::command::{
    Asked, Command, Context, Fetched, Given, Need, Source, Wanted, asked,
    homeless, run,
};
use home::issue::storage::File;
use home::{Draws, UnixMillis};
use home_files::FolderStore;
use home_store::{CanvasStore, StoreError};

mod local;
mod operations;
mod project;
mod tracking;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let (text, code) = operations::status(&args).unwrap_or_else(|| {
        root(&args)
            .as_deref()
            .map_or_else(homeless, |home| answered(home, &args))
    });
    let _written = std::io::stdout().lock().write_all(text.as_bytes());
    ExitCode::from(code)
}

fn answered(home: &Path, args: &[String]) -> (String, u8) {
    if let Some(answer) =
        tracking::answer(home, args).or_else(|| project::answer(home, args))
    {
        return answer;
    }
    let loaded = home_files::read(home);
    let snapshot = &loaded.snapshot;
    let mut store = FolderStore::new(home.to_path_buf());
    let context = context();
    let fresh = fresh(snapshot, context.now.secs(), &draws());
    match asked(args, snapshot, fresh) {
        Asked::Local(local) => local::run(home, local, snapshot),
        Asked::Flow(command) => {
            issues(home, &mut store, command, &loaded, &context)
        }
    }
}

fn issues(
    home: &Path,
    store: &mut dyn CanvasStore,
    asked: Command,
    loaded: &home::issue::storage::ReadHome,
    context: &Context,
) -> (String, u8) {
    let given = asked.needs().map(|need| match need {
        Need::Read(Source::File(path)) => Given::Text(read(Path::new(&path))),
        Need::Read(Source::Stdin) => Given::Text(stdin()),
        Need::Canvases(wanted) => Given::Canvases(fetched(&*store, &wanted)),
        Need::EveryCanvas => Given::EveryCanvas(every(&*store)),
    });
    let outcome = run(asked, given, &loaded.snapshot, context);
    let saved = outcome
        .changes
        .iter()
        .try_for_each(|change| {
            loaded.encode(change).and_then(|file| save(home, &file))
        })
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
) -> Result<Held<Fetched>, String> {
    let (names, strays) = if let Some(name) = &wanted.name {
        (Vec::from([name.clone()]), Vec::new())
    } else {
        let held = store
            .list(&wanted.project, &wanted.issue)
            .map_err(|error| error.to_string())?;
        (
            held.canvases
                .into_iter()
                .map(|kept| {
                    kept.map_or_else(|broken| broken.name, |entry| entry.name)
                })
                .collect(),
            held.strays,
        )
    };
    Ok(Held {
        canvases: names
            .into_iter()
            .map(|name| {
                got(
                    store,
                    CanvasKey {
                        project: wanted.project,
                        issue: wanted.issue.clone(),
                        name,
                    },
                )
            })
            .collect(),
        strays,
    })
}

fn every(store: &dyn CanvasStore) -> Result<Held<Fetched>, String> {
    let held = store.all().map_err(|error| error.to_string())?;
    Ok(Held {
        canvases: held
            .canvases
            .into_iter()
            .map(|key| got(store, key))
            .collect(),
        strays: held.strays,
    })
}

fn got(store: &dyn CanvasStore, key: CanvasKey) -> Fetched {
    let got = store.get(&key).map_err(|refused| match refused {
        StoreError::Invalid { error, .. } => error.to_string(),
        StoreError::Unavailable { reason } => reason,
    });
    Fetched { key, got }
}

fn stdin() -> Result<String, String> {
    let mut text = String::new();
    io::stdin()
        .read_to_string(&mut text)
        .map(|_| text)
        .map_err(|err| format!("stdin: {err}"))
}

fn root(args: &[String]) -> Option<PathBuf> {
    root_from(
        &env::current_dir().ok()?,
        home::backend::configuration_query(args),
    )
}

fn root_from(at: &Path, configuration_query: bool) -> Option<PathBuf> {
    at.ancestors()
        .find(|folder| {
            folder.join(PATH).is_file()
                || (configuration_query
                    && home_files::has_configuration(folder))
        })
        .map(Path::to_path_buf)
}

fn context() -> Context {
    let host = env::var("COMPUTERNAME")
        .or_else(|_| env::var("HOSTNAME"))
        .unwrap_or_else(|_| String::from("host"))
        .to_lowercase();
    Context {
        now: UnixMillis::since_epoch(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default(),
        ),
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

#[cfg(test)]
mod tests {
    #[test]
    fn a_configuration_query_finds_its_home_without_issue_flow()
    -> std::io::Result<()> {
        let root = std::env::temp_dir()
            .join(format!("home-config-discovery-{}", std::process::id()));
        let project = root.join("projects/a1b2c3");
        std::fs::create_dir_all(&project)?;
        std::fs::write(
            project.join("project.toml"),
            "name = \"local\"\ntitle = \"Local\"\nroot = \"fc27e50efb661d7ef4a45d3edbe0234ff0a6fbd5\"\n",
        )?;
        let found = super::root_from(&project, true);
        std::fs::remove_dir_all(&root)?;
        assert_eq!(found, Some(root));
        Ok(())
    }

    #[test]
    fn invalid_project_configuration_is_found_for_diagnostics()
    -> std::io::Result<()> {
        let root = std::env::temp_dir()
            .join(format!("home-invalid-discovery-{}", std::process::id()));
        let project = root.join("projects/a1b2c3");
        std::fs::create_dir_all(&project)?;
        std::fs::write(project.join("project.toml"), "invalid configuration")?;
        let found = super::root_from(&project, true);
        std::fs::remove_dir_all(&root)?;
        assert_eq!(found, Some(root));
        Ok(())
    }
}
