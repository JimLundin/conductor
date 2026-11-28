use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const FLOW: &str = include_str!("../../home/src/flow/test-flow.toml");
const FOLDER: &str = "7w85je";
const PROSE: &str = r#"{"title": "Why a store", "blocks": [{"prose": {"text": "The core decides; the store keeps."}}]}"#;

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _removed = fs::remove_dir_all(&self.0);
    }
}

fn git(at: &Path, args: &[&str]) -> std::io::Result<String> {
    let ran = Command::new("git")
        .arg("-C")
        .arg(at)
        .args(["-c", "user.name=test", "-c", "user.email=test@example.com"])
        .args(args)
        .output()?;
    assert!(
        ran.status.success(),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    Ok(String::from_utf8_lossy(&ran.stdout).into_owned())
}

fn ran(root: &Path, args: &[&str]) -> std::io::Result<(bool, String)> {
    let ran = Command::new(env!("CARGO_BIN_EXE_home"))
        .args(args)
        .current_dir(root)
        .env("HOME_BY", "devbox·test")
        .output()?;
    Ok((
        ran.status.success(),
        String::from_utf8_lossy(&ran.stdout).into_owned(),
    ))
}

fn home(root: &Path, args: &[&str]) -> std::io::Result<String> {
    let (succeeded, said) = ran(root, args)?;
    assert!(succeeded, "{said}");
    Ok(said)
}

fn committed_home(name: &str) -> std::io::Result<(Scratch, String)> {
    let scratch = Scratch(
        std::env::temp_dir()
            .join(format!("home-{name}-{}", std::process::id())),
    );
    let root = &scratch.0;
    let project = root.join("projects").join(FOLDER);
    fs::create_dir_all(&project)?;
    fs::create_dir_all(root.join("docs/agents"))?;
    fs::write(root.join("docs/agents/flow.toml"), FLOW)?;
    fs::write(
        project.join("project.toml"),
        "name = \"home\"\ntitle = \"Home\"\nroot = \"e22b7f7b2976650400965d4147ba771675f96076\"\n",
    )?;
    git(root, &["init", "-q", "-b", "main"])?;
    git(root, &["add", "."])?;
    git(root, &["commit", "-q", "-m", "first"])?;
    let opened = home(root, &["issue", "new", "home", "bug", "First"])?;
    let id = opened
        .trim_end()
        .strip_prefix("wrote home/")
        .unwrap_or_default()
        .to_owned();
    Ok((scratch, id))
}

fn issue(root: &Path, id: &str) -> std::io::Result<String> {
    fs::read_to_string(
        root.join("projects")
            .join(FOLDER)
            .join("issues")
            .join(format!("{id}.md")),
    )
}

#[test]
fn a_note_in_a_home_that_isnt_a_repository_fails_and_leaves_the_issue_unchanged()
-> std::io::Result<()> {
    let (scratch, id) = committed_home("unrepository")?;
    let root = &scratch.0;
    fs::remove_dir_all(root.join(".git"))?;
    let before = issue(root, &id)?;
    let note = ["issue", "note", "home", &id, "Seen", "on", "the", "devbox"];
    for _ in 0..2 {
        let (succeeded, said) = ran(root, &note)?;
        assert!(!succeeded, "{said}");
        assert_eq!(issue(root, &id)?, before, "{said}");
    }
    Ok(())
}

#[test]
fn a_note_whose_commit_fails_leaves_the_home_as_it_was() -> std::io::Result<()>
{
    let (scratch, id) = committed_home("uncommitted-note")?;
    let root = &scratch.0;
    let before = issue(root, &id)?;
    fs::write(root.join(".git/refs/heads/main.lock"), "")?;
    let (succeeded, said) = ran(root, &["issue", "note", "home", &id, "Seen"])?;
    assert!(!succeeded, "{said}");
    assert_eq!(issue(root, &id)?, before, "{said}");
    assert_eq!(git(root, &["status", "--porcelain"])?, "");
    Ok(())
}

#[test]
fn a_canvas_whose_commit_fails_leaves_the_home_as_it_was() -> std::io::Result<()>
{
    let (scratch, id) = committed_home("uncommitted-canvas")?;
    let root = &scratch.0;
    fs::write(root.join(".git/info/exclude"), "/why.json\n")?;
    fs::write(root.join("why.json"), PROSE)?;
    fs::write(root.join(".git/refs/heads/main.lock"), "")?;
    let (succeeded, said) = ran(
        root,
        &["canvas", "put", "home", &id, "why", "--file", "why.json"],
    )?;
    assert!(!succeeded, "{said}");
    assert!(
        !root
            .join("projects")
            .join(FOLDER)
            .join("issues")
            .join(&id)
            .exists(),
        "{said}"
    );
    assert_eq!(git(root, &["status", "--porcelain"])?, "");
    Ok(())
}

#[test]
fn a_note_and_a_canvas_on_a_markdown_issue_are_committed_to_the_home_signed_by_their_writer()
-> std::io::Result<()> {
    let scratch = Scratch(
        std::env::temp_dir()
            .join(format!("home-committed-{}", std::process::id())),
    );
    let root = &scratch.0;
    let project = root.join("projects").join(FOLDER);
    fs::create_dir_all(&project)?;
    fs::create_dir_all(root.join("docs/agents"))?;
    fs::write(root.join("docs/agents/flow.toml"), FLOW)?;
    fs::write(
        project.join("project.toml"),
        "name = \"home\"\ntitle = \"Home\"\nroot = \"e22b7f7b2976650400965d4147ba771675f96076\"\n",
    )?;
    git(root, &["init", "-q", "-b", "main"])?;
    git(root, &["add", "."])?;
    git(root, &["commit", "-q", "-m", "first"])?;
    let opened = home(root, &["issue", "new", "home", "bug", "First"])?;
    let id = opened
        .trim_end()
        .strip_prefix("wrote home/")
        .unwrap_or_default()
        .to_owned();
    home(
        root,
        &["issue", "note", "home", &id, "Seen", "on", "the", "devbox"],
    )?;
    fs::write(root.join("why.json"), PROSE)?;
    git(root, &["add", "why.json"])?;
    git(root, &["commit", "-q", "-m", "canvas source"])?;
    home(
        root,
        &["canvas", "put", "home", &id, "why", "--file", "why.json"],
    )?;
    assert_eq!(git(root, &["status", "--porcelain"])?, "");
    assert_eq!(
        git(root, &["log", "-4", "--format=%an|%s"])?,
        format!(
            "devbox·test|Write canvas why of issue {id}\ntest|canvas source\ndevbox·test|Note on issue {id}\ndevbox·test|Open issue {id}: First\n"
        )
    );
    let canvas = git(
        root,
        &[
            "show",
            &format!("HEAD:projects/{FOLDER}/issues/{id}/why.json"),
        ],
    )?;
    assert!(canvas.contains("Why a store"), "{canvas}");
    let kept = git(
        root,
        &["show", &format!("HEAD:projects/{FOLDER}/issues/{id}.md")],
    )?;
    assert!(kept.contains("Seen on the devbox"), "{kept}");
    Ok(())
}
