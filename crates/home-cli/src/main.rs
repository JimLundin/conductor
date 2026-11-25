use home::issue::storage::fresh;
use std::env;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use home::Draws;
use home::flow::PATH;
use home::flow::command::{Asked, Command, Context, asked, homeless, run};
use home::issue::storage::File;

mod local;
mod operations;
mod project;

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
    if let Some(answer) = project::answer(home, args) {
        return answer;
    }
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
