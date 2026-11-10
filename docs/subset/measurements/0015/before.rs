fn stun_schedule() -> Vec<Duration> {
    std::iter::once(Duration::ZERO)
        .chain(std::iter::once(Duration::from_millis(250)))
        .chain(std::iter::once(Duration::from_millis(750)))
        .chain(std::iter::once(Duration::from_millis(1750)))
        .collect()
}

fn git_init(scratch: &Path) -> std::io::Result<ExitStatus> {
    Command::new("git")
        .args(std::iter::once("init").chain(std::iter::once("-q")))
        .current_dir(scratch)
        .status()
}

fn secret_key(bytes: Vec<u8>) -> Option<SecretKey> {
    bytes.try_into().ok().map(|bytes| SecretKey::from_bytes(&bytes))
}

fn options(claude: PathBuf) -> ClaudeOptions {
    ClaudeOptions {
        setting_sources: Some(
            std::iter::once(SettingSource::Project)
                .chain(std::iter::once(SettingSource::Local))
                .collect(),
        ),
        ..ClaudeOptions::new(claude)
    }
}
