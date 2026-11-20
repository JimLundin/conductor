use std::path::{Path, PathBuf};

pub type TestResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/claude")
        .join(name)
}

pub fn test_dir(name: &str) -> TestResult<PathBuf> {
    let dir = std::env::current_exe()?
        .ancestors()
        .nth(3)
        .ok_or("the test binary is not under target/<profile>/deps")?
        .join("tmp")
        .join(format!("agents-replay-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn fake_claude(name: &str, stdout: &Path) -> TestResult<PathBuf> {
    let dir = test_dir(name)?;
    std::fs::copy(stdout, dir.join("stdout.jsonl"))?;
    Ok(dir)
}

pub fn fake_cli_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake-claude.sh")
}
