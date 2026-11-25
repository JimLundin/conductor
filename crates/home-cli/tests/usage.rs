use std::fs;
use std::path::PathBuf;
use std::process::Command;

const FLOW: &str = include_str!("../../home/src/flow/test-flow.toml");

const COMMANDS: [&str; 22] = [
    "home project access <project> [host-data-dir]",
    "home project capabilities <project>",
    "home project operation-status <handle> [--data-dir <dir>]",
    "home check",
    "home issue list <project> [--offline] [--data-dir <dir>]",
    "home issue show <project> <reference> [--offline] [--data-dir <dir>]",
    "home issue new <project> <kind> <title> [<field>=<value>...]",
    "home issue move <id> stage|status <to> [--signed-off] [<reason>...]",
    "home issue set <id> <field>=<value>...",
    "home issue note <id> <text>...",
    "home issue body <id> <text>...",
    "home issue body <id> --file <path>",
    "home issue checklist <id> add <text>...",
    "home issue checklist <id> check|uncheck|remove <n>",
    "home canvas put <id> <name> [--file <path>]",
    "home canvas show <id> [<name>]",
    "home canvas schema",
    "home repos",
    "home repos --verify",
    "home repos sync",
    "home repos scan",
    "home skills",
];

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _removed = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn home_alone_says_every_command_with_its_meaning() -> std::io::Result<()> {
    let home = Scratch(
        std::env::temp_dir()
            .join(format!("home-cli-usage-{}", std::process::id())),
    );
    fs::create_dir_all(home.0.join("docs/agents"))?;
    fs::write(home.0.join("docs/agents/flow.toml"), FLOW)?;
    let ran = Command::new(env!("CARGO_BIN_EXE_home"))
        .current_dir(&home.0)
        .output()?;
    let said = String::from_utf8_lossy(&ran.stdout);
    let lines: Vec<&str> = said.lines().collect();
    let commands: Vec<&str> = lines
        .iter()
        .filter_map(|line| line.strip_prefix("  home ").map(|_| line.trim()))
        .collect();
    assert_eq!(commands, COMMANDS, "{said}");
    for (at, line) in lines.iter().enumerate() {
        let next = lines.get(at + 1).copied().unwrap_or_default();
        if line.starts_with("  home ") && !next.starts_with("  home ") {
            assert!(
                next.starts_with("      ") && !next.trim().is_empty(),
                "{line:?} has no meaning under it: {said}"
            );
        }
    }
    Ok(())
}
