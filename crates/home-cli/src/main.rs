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
    Asked, Command, Context, Fetched, Given, Need, Outcome, Source, Wanted,
    asked, homeless, run,
};
use home::issue::storage::ReadHome;
use home::{Draws, IssueId, UnixMillis};
use home_store::{CanvasStore, StoreError};

#[cfg(test)]
mod edited;
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
    let loaded = home_files::read(home, github::ISSUERS);
    let snapshot = &loaded.snapshot;
    let context = context();
    let fresh = fresh(snapshot, context.now.secs(), &draws());
    match asked(args, snapshot, fresh) {
        Asked::Local(local) => local::run(home, local, snapshot),
        Asked::Flow(command) => tracked(home, loaded, command, &context),
    }
}

fn tracked(
    home: &Path,
    loaded: ReadHome,
    command: Command,
    context: &Context,
) -> (String, u8) {
    let data = command.data().map(PathBuf::from);
    match Trackers::new(home, loaded, data, resolve::connect) {
        Ok(mut trackers) => issues(home, &mut trackers, command, context),
        Err(reason) => (format!("home: {reason}\n"), 1),
    }
}

fn issues(
    home: &Path,
    trackers: &mut Trackers,
    asked: Command,
    context: &Context,
) -> (String, u8) {
    if let Err(reason) = trackers.catalog() {
        return (format!("home: {reason}\n"), 1);
    }
    let store = trackers.stores();
    let given = asked.needs().map(|need| match need {
        Need::Read(Source::File(path)) => Given::Text(read(Path::new(&path))),
        Need::Read(Source::Stdin) => Given::Text(stdin()),
        Need::Canvases(wanted) => Given::Canvases(fetched(store, &wanted)),
        Need::EveryCanvas => Given::EveryCanvas(every(store)),
    });
    let resolved =
        trackers.resolved(asked.issues(given.as_ref()), asked.mode());
    let mut outcome = run(
        asked,
        given,
        &resolved,
        &trackers.loaded().snapshot,
        context,
    );
    let saved = kept(home, trackers, &outcome, context)
        .map(|ids| outcome.named(&trackers.loaded().snapshot, &ids));
    outcome.ended(saved)
}

fn kept(
    home: &Path,
    trackers: &mut Trackers,
    outcome: &Outcome,
    context: &Context,
) -> Result<Vec<IssueId>, String> {
    let places = trackers.touches(&outcome.changes, &outcome.canvases)?;
    let record = outcome
        .commit_message(&places)
        .map(|subject| opened(home).map(|repository| (repository, subject)))
        .transpose()?;
    let before = places
        .iter()
        .map(|place| prior(home, place))
        .collect::<Result<Vec<Prior>, String>>()?;
    written(home, trackers, outcome)
        .and_then(|(paths, ids)| {
            record.map_or(Ok(()), |(repository, subject)| {
                repository
                    .commit(&paths, &context.by, subject)
                    .map(drop)
                    .map_err(|error| format!("committing the home: {error}"))
            })?;
            Ok(ids)
        })
        .map_err(|failed| restored(home, &before, failed))
}

fn opened(home: &Path) -> Result<vcs_git2::record::Home, String> {
    vcs_git2::record::open(home)
        .map_err(|error| format!("opening the home's repository: {error}"))
}

fn written(
    home: &Path,
    trackers: &mut Trackers,
    outcome: &Outcome,
) -> Result<(Vec<String>, Vec<IssueId>), String> {
    let written = trackers.write(home, &outcome.changes)?;
    let ids = written.iter().map(|wrote| wrote.id.clone()).collect();
    let mut paths: Vec<String> =
        written.into_iter().filter_map(|wrote| wrote.file).collect();
    paths.extend(trackers.put(&outcome.canvases)?);
    Ok((paths, ids))
}

fn restored(home: &Path, before: &[Prior], failed: String) -> String {
    match before.iter().try_for_each(|kept| restore(home, kept)) {
        Ok(()) => failed,
        Err(undone) => format!("{failed}; restoring the home: {undone}"),
    }
}

struct Prior {
    place: String,
    bytes: Option<Vec<u8>>,
    made: Option<PathBuf>,
}

fn prior(root: &Path, place: &str) -> Result<Prior, String> {
    let path = root.join(place);
    match fs::read(&path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
    .map(|bytes| Prior {
        place: place.to_owned(),
        bytes,
        made: made(root, place),
    })
}

fn made(root: &Path, place: &str) -> Option<PathBuf> {
    Path::new(place)
        .ancestors()
        .skip(1)
        .take_while(|folder| {
            !folder.as_os_str().is_empty() && !root.join(folder).exists()
        })
        .last()
        .map(|folder| root.join(folder))
}

fn restore(root: &Path, prior: &Prior) -> Result<(), String> {
    let path = root.join(&prior.place);
    prior
        .bytes
        .as_ref()
        .map_or_else(
            || gone(fs::remove_file(&path)),
            |bytes| fs::write(&path, bytes),
        )
        .map_err(|error| format!("{}: {error}", path.display()))?;
    prior.made.as_ref().map_or(Ok(()), |folder| {
        gone(fs::remove_dir_all(folder))
            .map_err(|error| format!("{}: {error}", folder.display()))
    })
}

fn gone(removing: io::Result<()>) -> io::Result<()> {
    match removing {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        removed => removed,
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
        unavailable: Vec::new(),
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
        unavailable: held.unavailable,
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use home::canvas::{Stray, StrayReason};
    use home::flow::command::{Asked, asked};
    use home::issue::ReadFailure;
    use home::issue::storage::fresh;
    use home_store::Memory;
    use project_runtime::tracker::{Connection, Connector, Reader};

    use super::resolve::Trackers;

    const FLOW: &str = include_str!("../../home/src/flow/test-flow.toml");

    struct Offline;

    impl Connector for Offline {
        fn connect(&self) -> Result<Reader, ReadFailure> {
            Err(ReadFailure::Unavailable)
        }
    }

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
        let loaded = home_files::read(&root, github::ISSUERS);
        let context = super::context();
        let fresh =
            fresh(&loaded.snapshot, context.now.secs(), &[7; home::DRAWS]);
        let Asked::Flow(command) =
            asked(&[String::from("check")], &loaded.snapshot, fresh)
        else {
            return Err("check isn't a flow command".into());
        };
        let mut trackers =
            Trackers::new(&root, loaded, None, |_, _, project| {
                let mut github = Memory::default();
                github.stray(
                    project.id,
                    None,
                    Stray {
                        place: String::from("issues/146#issuecomment-7"),
                        reason: StrayReason::Unnamed,
                    },
                );
                Ok(Connection {
                    canvases: Box::new(github),
                    connector: Arc::new(Offline),
                })
            })?;
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
    fn a_tracker_whose_mapping_fails_to_load_fails_every_canvas_command_naming_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir()
            .join(format!("home-unmapped-{}", std::process::id()));
        let project = root.join("projects/hyn162");
        std::fs::create_dir_all(&project)?;
        std::fs::create_dir_all(root.join("docs/agents"))?;
        std::fs::write(root.join("docs/agents/flow.toml"), FLOW)?;
        std::fs::write(
            project.join("project.toml"),
            "name = \"standards\"\ntitle = \"Standards\"\nroot = \"e22b7f7b2976650400965d4147ba771675f96076\"\n[backends.tracker]\nadapter = \"github\"\nresource = \"owner/standards\"\noptions = 'colour = \"blue\"'\n",
        )?;
        let canvas = root.join("design.json");
        std::fs::write(
            &canvas,
            "{\"title\":\"Kept by its tracker\",\"blocks\":[{\"kind\":\"prose\",\"text\":\"The tracker keeps it.\"}]}",
        )?;
        let file = canvas.to_string_lossy().into_owned();
        let said = [
            Vec::from([
                "canvas",
                "put",
                "standards",
                "146",
                "design",
                "--file",
                &file,
            ]),
            Vec::from(["canvas", "show", "standards", "146", "design"]),
            Vec::from(["canvas", "show", "standards", "146"]),
        ]
        .map(|words| {
            let args: Vec<String> =
                words.into_iter().map(String::from).collect();
            let loaded = home_files::read(&root, github::ISSUERS);
            let context = super::context();
            let fresh =
                fresh(&loaded.snapshot, context.now.secs(), &[7; home::DRAWS]);
            let Asked::Flow(command) = asked(&args, &loaded.snapshot, fresh)
            else {
                return (String::from("not a flow command"), 0);
            };
            Trackers::new(
                &root,
                loaded,
                Some(root.join("data")),
                super::resolve::connect,
            )
            .map_or_else(
                |reason| (reason, 1),
                |mut trackers| {
                    super::issues(&root, &mut trackers, command, &context)
                },
            )
        });
        let kept = std::fs::read_dir(&project)?
            .map(|found| found.map(|entry| entry.file_name()))
            .collect::<Result<Vec<_>, _>>()?;
        std::fs::remove_dir_all(&root)?;
        for (text, code) in &said {
            assert!(text.contains("unknown field `colour`"), "{text}");
            assert_eq!(*code, 1, "{text}");
        }
        assert_eq!(kept, Vec::from([std::ffi::OsString::from("project.toml")]));
        Ok(())
    }

    #[test]
    fn check_fails_naming_a_project_whose_tracker_cant_be_built()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir()
            .join(format!("home-check-unbuilt-{}", std::process::id()));
        let project = root.join("projects/hyn162");
        std::fs::create_dir_all(&project)?;
        std::fs::create_dir_all(root.join("docs/agents"))?;
        std::fs::write(root.join("docs/agents/flow.toml"), FLOW)?;
        std::fs::write(
            project.join("project.toml"),
            "name = \"standards\"\ntitle = \"Standards\"\nroot = \"e22b7f7b2976650400965d4147ba771675f96076\"\n[backends.tracker]\nadapter = \"github\"\nresource = \"owner/standards\"\n",
        )?;
        let loaded = home_files::read(&root, github::ISSUERS);
        let context = super::context();
        let fresh =
            fresh(&loaded.snapshot, context.now.secs(), &[7; home::DRAWS]);
        let Asked::Flow(command) =
            asked(&[String::from("check")], &loaded.snapshot, fresh)
        else {
            return Err("check isn't a flow command".into());
        };
        let mut trackers = Trackers::new(&root, loaded, None, |_, _, _| {
            Err(String::from("unknown field `colour`"))
        })?;
        let (said, code) =
            super::issues(&root, &mut trackers, command, &context);
        std::fs::remove_dir_all(&root)?;
        assert!(
            said.contains(
                "error  the tracker of standards: unknown field `colour`\n"
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
