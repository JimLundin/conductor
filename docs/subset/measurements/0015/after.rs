const STUN_SCHEDULE: [Duration; 4] = [
    Duration::ZERO,
    Duration::from_millis(250),
    Duration::from_millis(750),
    Duration::from_millis(1750),
];

fn git_init(scratch: &Path) -> std::io::Result<ExitStatus> {
    Command::new("git").args(["init", "-q"]).current_dir(scratch).status()
}

fn secret_key(bytes: [u8; 32]) -> SecretKey {
    SecretKey::from_bytes(&bytes)
}

fn options(claude: PathBuf) -> ClaudeOptions {
    ClaudeOptions {
        setting_sources: Some(Vec::from([SettingSource::Project, SettingSource::Local])),
        ..ClaudeOptions::new(claude)
    }
}
