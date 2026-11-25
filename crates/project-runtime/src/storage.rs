use std::fs::{File, OpenOptions};
use std::path::Path;

pub const CACHE_BYTES: usize = 1024 * 1024;

pub fn private(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}
