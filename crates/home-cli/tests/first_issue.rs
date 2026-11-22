//! The `home` command line run in a home on disk.

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
    let ran = Command::new(env!("CARGO_BIN_EXE_home"))
        .args(["issue", "new", "home", "bug", "First"])
        .current_dir(&home.0)
        .output()?;
    let said = String::from_utf8_lossy(&ran.stdout);
    assert!(ran.status.success(), "{said}");
    let written: Vec<_> = fs::read_dir(project.join("issues"))?.collect();
    assert_eq!(written.len(), 1, "{said}");
    let id = said
        .trim_end()
        .strip_prefix("wrote home/")
        .unwrap_or_default();
    assert_eq!(id.len(), 8, "{said}");
    assert!(project.join("issues").join(format!("{id}.md")).is_file());
    Ok(())
}
