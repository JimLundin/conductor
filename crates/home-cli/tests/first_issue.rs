use std::fs;
use std::path::PathBuf;
use std::process::Command;

const FLOW: &str = include_str!("../../home/src/flow/test-flow.toml");
const FOLDER: &str = "7w85je";

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _removed = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_first_issue_of_a_project_makes_its_issues_folder() -> std::io::Result<()>
{
    let home = Scratch(
        std::env::temp_dir().join(format!("home-cli-{}", std::process::id())),
    );
    let project = home.0.join("projects").join(FOLDER);
    fs::create_dir_all(&project)?;
    fs::create_dir_all(home.0.join("docs/agents"))?;
    fs::write(home.0.join("docs/agents/flow.toml"), FLOW)?;
    fs::write(
        project.join("project.toml"),
        "name = \"home\"\ntitle = \"Home\"\nroot = \"e22b7f7b2976650400965d4147ba771675f96076\"\n",
    )?;
    let init = Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .arg(&home.0)
        .status()?;
    assert!(init.success());
    let ran = Command::new(env!("CARGO_BIN_EXE_home"))
        .args(["issue", "new", "home", "bug", "First"])
        .current_dir(&home.0)
        .env("HOME", &home.0)
        .env("XDG_CONFIG_HOME", &home.0)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()?;
    let said = String::from_utf8_lossy(&ran.stdout);
    assert!(ran.status.success(), "{said}");
    let written: Vec<_> = fs::read_dir(project.join("issues"))?.collect();
    assert_eq!(written.len(), 1, "{said}");
    let id = said
        .trim_end()
        .strip_prefix("wrote home ")
        .unwrap_or_default();
    assert_eq!(id.len(), 8, "{said}");
    assert!(
        id.bytes()
            .all(|digit| b"0123456789abcdefghjkmnpqrstvwxyz".contains(&digit)),
        "{said}"
    );
    assert!(project.join("issues").join(format!("{id}.md")).is_file());
    Ok(())
}

#[test]
fn configured_issue_reads_report_cache_coverage_and_opening_an_issue_there_needs_a_credential()
-> Result<(), Box<dyn std::error::Error>> {
    let home = Scratch(
        std::env::temp_dir()
            .join(format!("home-tracked-{}", std::process::id())),
    );
    let project = home.0.join("projects").join(FOLDER);
    fs::create_dir_all(&project)?;
    let catalog = project.join("project.toml");
    let local = "name = \"sample\"\ntitle = \"Sample\"\nroot = \"e22b7f7b2976650400965d4147ba771675f96076\"\n";
    fs::write(&catalog, local)?;
    let cli = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_home"))
            .current_dir(&project)
            .args(args)
            .output()
    };
    let markdown = cli(&["issue", "list", "sample"])?;
    assert!(markdown.status.success());
    let listed: serde_json::Value = serde_json::from_slice(&markdown.stdout)?;
    assert_eq!(listed["records"], serde_json::json!([]));
    assert_eq!(listed["status"]["complete"], true);

    fs::create_dir_all(home.0.join("docs/agents"))?;
    fs::write(home.0.join("docs/agents/flow.toml"), FLOW)?;
    fs::write(
        catalog,
        format!(
            "{local}[backends.tracker]\nadapter = \"github\"\nresource = \"owner/issues\"\n"
        ),
    )?;
    let rejected = cli(&["issue", "new", "sample", "bug", "Report"])?;
    assert!(!rejected.status.success());
    let refusal = String::from_utf8(rejected.stdout)?;
    assert_eq!(refusal, "home: host credential is missing\n");
    assert!(!project.join("issues").exists());

    let online = cli(&["issue", "list", "sample", "--data-dir", "host"])?;
    assert!(!online.status.success());
    let report: serde_json::Value = serde_json::from_slice(&online.stdout)?;
    assert_eq!(report["records"], serde_json::json!([]));
    assert_eq!(report["status"]["failure"], "CredentialMissing");
    assert_eq!(report["status"]["complete"], false);
    let offline = cli(&[
        "issue",
        "show",
        "sample",
        "42",
        "--data-dir",
        "host",
        "--offline",
    ])?;
    assert!(!offline.status.success());
    assert_eq!(
        String::from_utf8(offline.stdout)?,
        "home: no issue 42 in sample is cached; `home issue list sample` refreshes the cache\n"
    );
    Ok(())
}
