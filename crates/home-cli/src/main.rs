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
mod resolve;
mod tracking;

use resolve::Trackers;

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
    let context = context();
    let fresh = fresh(snapshot, context.now.secs(), &draws());
    match asked(args, snapshot, fresh) {
        Asked::Local(local) => local::run(home, local, snapshot),
        Asked::Flow(command) => {
            let data = command.data().map(PathBuf::from);
            let mut trackers = Trackers::new(home, loaded, data);
            issues(home, &mut trackers, command, &context)
        }
    }
}

fn issues(
    home: &Path,
    trackers: &mut Trackers<'_>,
    asked: Command,
    context: &Context,
) -> (String, u8) {
    let store = trackers.stores();
    let given = asked.needs().map(|need| match need {
        Need::Read(Source::File(path)) => Given::Text(read(Path::new(&path))),
        Need::Read(Source::Stdin) => Given::Text(stdin()),
        Need::Canvases(wanted) => Given::Canvases(fetched(store, &wanted)),
        Need::EveryCanvas => Given::EveryCanvas(every(store)),
    });
    let resolved =
        trackers.resolved(asked.issues(given.as_ref()), asked.mode());
    let loaded = trackers.loaded();
    let outcome = run(asked, given, &resolved, &loaded.snapshot, context);
    let written = outcome
        .changes
        .iter()
        .map(|change| {
            let file = loaded.encode(change)?;
            save(home, &file).map(|()| file.path())
        })
        .collect::<Result<Vec<String>, String>>();
    let kept: Vec<String> = outcome
        .canvases
        .iter()
        .filter(|canvas| trackers.keeps(canvas.key.project))
        .map(|canvas| FolderStore::place(&canvas.key))
        .collect();
    let saved = written.and_then(|mut paths| {
        trackers.put(&outcome.canvases)?;
        paths.extend(kept);
        committed(home, &paths, outcome.message.as_deref(), context)
    });
    outcome.ended(saved)
}

fn committed(
    home: &Path,
    paths: &[String],
    message: Option<&str>,
    context: &Context,
) -> Result<(), String> {
    match message {
        Some(subject) if !paths.is_empty() => {
            vcs_git2::record::commit(home, paths, &context.by, subject)
                .map(drop)
                .map_err(|error| format!("committing the home: {error}"))
        }
        _ => Ok(()),
    }
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
        StoreError::Unavailable { reason }
        | StoreError::Refused { reason, .. } => reason,
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
    use home::ProjectId;
    use home::canvas::{Stray, StrayReason};
    use home::flow::command::{Asked, asked};
    use home::issue::storage::fresh;
    use home_store::Memory;

    use super::resolve::Trackers;

    const FLOW: &str = include_str!("../../home/src/flow/test-flow.toml");

    #[test]
    fn check_reports_a_stray_from_a_tracker_s_store_as_an_error()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir()
            .join(format!("home-tracker-stray-{}", std::process::id()));
        let project = root.join("projects/hyn162");
        std::fs::create_dir_all(&project)?;
        std::fs::create_dir_all(root.join("docs/agents"))?;
        std::fs::write(root.join("docs/agents/flow.toml"), FLOW)?;
        std::fs::write(
            project.join("project.toml"),
            "name = \"standards\"\ntitle = \"Standards\"\nroot = \"e22b7f7b2976650400965d4147ba771675f96076\"\n[backends.tracker]\nadapter = \"github\"\nresource = \"owner/standards\"\n",
        )?;
        let tracked = ProjectId::parse("hyn162").ok_or("project")?;
        let mut github = Memory::default();
        github.stray(
            tracked,
            None,
            Stray {
                place: String::from("issues/146#issuecomment-7"),
                reason: StrayReason::Unnamed,
            },
        );
        let loaded = home_files::read(&root);
        let context = super::context();
        let fresh =
            fresh(&loaded.snapshot, context.now.secs(), &[7; home::DRAWS]);
        let Asked::Flow(command) =
            asked(&[String::from("check")], &loaded.snapshot, fresh)
        else {
            return Err("check isn't a flow command".into());
        };
        let mut trackers = Trackers::new(&root, loaded, None);
        trackers.stores_mut().track(tracked, Box::new(github));
        let (said, code) =
            super::issues(&root, &mut trackers, command, &context);
        std::fs::remove_dir_all(&root)?;
        assert!(
            said.contains(
                "error  issues/146#issuecomment-7  isn't a canvas: name it in lowercase words joined by `-`, or move it out of the issue\n"
            ),
            "{said}"
        );
        assert_eq!(code, 1, "{said}");
        Ok(())
    }

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
