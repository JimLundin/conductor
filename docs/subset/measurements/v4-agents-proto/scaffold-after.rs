// crates/agents/src/live.rs, `pub mod live;` in lib.rs, shared by s1 and s2.
mod live {
#[derive(Debug, Clone, Copy)]
pub enum Presence {
    Present,
    Missing,
}
pub fn presence(path: &Path) -> Presence {
    match std::fs::metadata(path) {
        Ok(_) => Presence::Present,
        Err(_) => Presence::Missing,
    }
}
#[derive(Clone, Copy)]
pub enum Verdict {
    Pass,
    Fail,
}
pub fn report(step: &str, verdict: Verdict, detail: String) -> Result<Verdict> {
    let label = match verdict {
        Verdict::Pass => "PASS",
        Verdict::Fail => "FAIL",
    };
    writeln!(std::io::stdout(), "{label} {step}: {detail}")?;
    Ok(verdict)
}
pub fn log_path(fixtures: Option<&Path>, name: &str) -> Option<PathBuf> {
    fixtures.map(|dir| {
        let path = dir.join(format!("{name}.jsonl"));
        let _ = std::fs::remove_file(&path);
        path
    })
}
pub fn fresh_scratch(scratch: &Path) -> Result<()> {
    if let Presence::Present = presence(scratch) {
        std::fs::remove_dir_all(scratch)?;
    }
    std::fs::create_dir_all(scratch)?;
    let git = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(scratch)
        .status()?;
    match git.code() {
        Some(0) => Ok(()),
        _ => Err(anyhow::Error::msg(format!("git init failed in {}", scratch.display()))),
    }
}
}
mod s1 {
    use agents::live::{Presence, Verdict, fresh_scratch, log_path, presence, report};
    fn call() { fresh_scratch(Path::new(SCRATCH))?; }
}
mod s2 {
    use agents::live::{Presence, Verdict, fresh_scratch, log_path, presence, report};
    fn call() { fresh_scratch(Path::new(SCRATCH))?; }
}
