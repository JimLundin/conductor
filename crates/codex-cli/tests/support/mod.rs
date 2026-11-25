use std::path::{Path, PathBuf};

pub type TestResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/codex")
        .join(name)
}

pub fn fake_codex_dir(fixture: &Path, replay: &str) -> TestResult<PathBuf> {
    let stem = fixture
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or("the fixture has no UTF-8 file stem")?;
    let dir = std::env::current_exe()?
        .ancestors()
        .nth(3)
        .ok_or("the test binary is not under target/<profile>/deps")?
        .join("tmp")
        .join(format!(
            "agents-codex-replay-{}-{stem}-{replay}",
            std::process::id()
        ));
    std::fs::create_dir_all(&dir)?;
    std::fs::copy(fixture, dir.join("stdout.jsonl"))?;
    Ok(dir)
}

pub fn committed_fake_cli_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake-codex.sh")
}
