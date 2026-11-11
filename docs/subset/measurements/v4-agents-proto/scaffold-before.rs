mod s1 {
#[derive(Debug, Clone, Copy)]
enum Presence {
    Present,
    Missing,
}
fn presence(path: &Path) -> Presence {
    match std::fs::metadata(path) {
        Ok(_) => Presence::Present,
        Err(_) => Presence::Missing,
    }
}
#[derive(Clone, Copy)]
enum Verdict {
    Pass,
    Fail,
}
fn report(step: &str, verdict: Verdict, detail: String) -> Result<Verdict> {
    let label = match verdict {
        Verdict::Pass => "PASS",
        Verdict::Fail => "FAIL",
    };
    writeln!(std::io::stdout(), "{label} {step}: {detail}")?;
    Ok(verdict)
}
fn log_path(fixtures: Option<&Path>, name: &str) -> Option<PathBuf> {
    fixtures.map(|dir| {
        let path = dir.join(format!("{name}.jsonl"));
        let _ = std::fs::remove_file(&path);
        path
    })
}
fn fresh_scratch() -> Result<()> {
    let scratch = Path::new(SCRATCH);
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
        _ => Err(anyhow::Error::msg(format!("git init failed in {SCRATCH}"))),
    }
}
}
mod s2 {
#[derive(Debug, Clone, Copy)]
enum Presence {
    Present,
    Missing,
}
fn presence(path: &Path) -> Presence {
    match std::fs::metadata(path) {
        Ok(_) => Presence::Present,
        Err(_) => Presence::Missing,
    }
}
#[derive(Clone, Copy)]
enum Verdict {
    Pass,
    Fail,
}
fn report(step: &str, verdict: Verdict, detail: String) -> Result<Verdict> {
    let label = match verdict {
        Verdict::Pass => "PASS",
        Verdict::Fail => "FAIL",
    };
    writeln!(std::io::stdout(), "{label} {step}: {detail}")?;
    Ok(verdict)
}
fn log_path(fixtures: Option<&Path>, name: &str) -> Option<PathBuf> {
    fixtures.map(|dir| {
        let path = dir.join(format!("{name}.jsonl"));
        let _ = std::fs::remove_file(&path);
        path
    })
}
fn fresh_scratch() -> Result<()> {
    let scratch = Path::new(SCRATCH);
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
        _ => Err(anyhow::Error::msg(format!("git init failed in {SCRATCH}"))),
    }
}
}
