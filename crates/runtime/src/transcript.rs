//! Portable transcript files exchanged by the host's provider plugins.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub use proto::transcript::{
    MAX_TRANSCRIPT_BYTES, MAX_TRANSCRIPT_FILES, Transcript, TranscriptFile,
};
use proto::transcript::{portable_component, portable_path};

/// Decodes a portable path of normal components, independent of the target OS.
///
/// # Errors
/// The path is empty, absolute, too long or contains an unsafe name.
pub fn relative(name: &str) -> io::Result<PathBuf> {
    if !portable_path(name) {
        return Err(io::Error::other("invalid transcript path"));
    }
    Ok(name.split('/').collect())
}

/// Opens a directory without following a link in its final component.
///
/// # Errors
/// The directory is missing, unreadable or is a link.
pub fn directory(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.file_type().is_dir() {
        Ok(())
    } else {
        Err(io::Error::other("transcript path must be a real directory"))
    }
}

/// Creates a child directory after validating its parent. An existing
/// directory is accepted, including one created by a prior attempt.
///
/// # Errors
/// The parent or child is invalid or cannot be created.
pub fn create_directory(parent: &Path, component: &str) -> io::Result<PathBuf> {
    portable_component(component)
        .then_some(())
        .ok_or_else(|| io::Error::other("invalid transcript directory name"))?;
    directory(parent)?;
    let path = parent.join(component);
    match fs::create_dir(&path) {
        Ok(()) => sync_directory(parent)?,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            directory(&path)?;
        }
        Err(error) => return Err(error),
    }
    Ok(path)
}

/// Collects a single regular file into the archive without reading above the
/// remaining byte budget.
///
/// # Errors
/// The file is missing, unreadable, is a link or exceeds the archive limit.
pub fn read_file(
    path: &Path,
    name: String,
    remaining: &mut usize,
) -> io::Result<TranscriptFile> {
    let _ = relative(&name)?;
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(io::Error::other("transcript entry must be a real file"));
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(
            u64::try_from(*remaining)
                .unwrap_or(u64::MAX)
                .saturating_add(1),
        )
        .read_to_end(&mut bytes)?;
    *remaining = remaining
        .checked_sub(bytes.len())
        .ok_or_else(|| io::Error::other("transcript exceeds byte limit"))?;
    Ok(TranscriptFile { path: name, bytes })
}

/// Flushes a directory entry after publishing a transcript.
///
/// # Errors
/// The directory cannot be opened or synced.
#[cfg(unix)]
pub fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

/// # Errors
/// Always succeeds on Windows. Transcript files themselves are flushed before renaming.
#[cfg(not(unix))]
pub const fn sync_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}
