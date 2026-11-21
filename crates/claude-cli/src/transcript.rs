//! Claude stores a managed session's transcripts and memory under its
//! Conductor ULID, regardless of the native UUID or working directory.

use std::fs::{self, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};

use claude::transcript::{allowed, validate, validate_ids};
use proto::SessionId;
use runtime::transcript::{
    MAX_TRANSCRIPT_FILES, create_directory, directory, read_file, relative,
    sync_directory,
};
use runtime::{MAX_TRANSCRIPT_BYTES, Transcript, TranscriptFiles};

pub const FILES: TranscriptFiles = TranscriptFiles {
    export: |home, session, ids| {
        export(home, session, ids).map_err(|error| error.to_string())
    },
    import: |home, session, ids, archive| {
        import(home, session, ids, archive).map_err(|error| error.to_string())
    },
    remove: |home, session, ids| {
        remove(home, session, ids).map_err(|error| error.to_string())
    },
};

fn session_project(session: SessionId) -> String {
    ulid::Ulid::from(session.0).to_string()
}

fn export(
    home: &Path,
    session: SessionId,
    ids: &[String],
) -> io::Result<Transcript> {
    directory(home)?;
    let projects = home.join("projects");
    directory(&projects)?;
    let project = projects.join(session_project(session));
    let archive = collect(&project)?;
    validate(&archive, ids).map_err(io::Error::other)?;
    Ok(archive)
}

struct Collection {
    archive: Transcript,
    pending: Vec<(PathBuf, String)>,
    remaining: usize,
}

fn collect(project: &Path) -> io::Result<Transcript> {
    directory(project)?;
    let mut contents = Collection {
        archive: Transcript { files: Vec::new() },
        remaining: MAX_TRANSCRIPT_BYTES,
        pending: vec![(project.to_path_buf(), String::new())],
    };
    while let Some((path, prefix)) = contents.pending.pop() {
        for entry in fs::read_dir(path)? {
            contents.entry(&entry?, &prefix)?;
        }
    }
    contents.archive.files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(contents.archive)
}

impl Collection {
    fn entry(&mut self, entry: &fs::DirEntry, prefix: &str) -> io::Result<()> {
        let component = entry.file_name().into_string().map_err(|_| {
            io::Error::other("Claude transcript name is not portable")
        })?;
        let name = if prefix.is_empty() {
            component
        } else {
            format!("{prefix}/{component}")
        };
        let _ = relative(&name)?;
        let path = entry.path();
        let is_directory = entry.file_type()?.is_dir();
        check_entry(&name, is_directory)?;
        if is_directory {
            self.pending.push((path, name));
            Ok(())
        } else {
            self.file(&path, name)
        }
    }

    fn file(&mut self, path: &Path, name: String) -> io::Result<()> {
        if self.archive.files.len() >= MAX_TRANSCRIPT_FILES {
            return Err(io::Error::other(
                "Claude transcript file limit exceeded",
            ));
        }
        self.archive
            .files
            .push(read_file(path, name, &mut self.remaining)?);
        Ok(())
    }
}

fn check_entry(name: &str, directory: bool) -> io::Result<()> {
    allowed(name, !directory)
        .then_some(())
        .ok_or_else(|| io::Error::other("unrecognized Claude project entry"))
}

fn import(
    home: &Path,
    session: SessionId,
    ids: &[String],
    archive: &Transcript,
) -> io::Result<()> {
    validate(archive, ids).map_err(io::Error::other)?;
    let projects = create_directory(home, "projects")?;
    let project = projects.join(session_project(session));
    if directory_exists(&project)? {
        return same_project(&project, archive);
    }
    let staging = tempfile::Builder::new()
        .prefix(".conductor-transcript-")
        .tempdir_in(&projects)?;
    for file in &archive.files {
        stage_file(staging.path(), file)?;
    }
    publish(staging.path(), &project, &projects, archive)
}

fn stage_file(
    staging: &Path,
    file: &runtime::TranscriptFile,
) -> io::Result<()> {
    let path = staged_path(staging, &file.path)?;
    let mut written = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    written.write_all(&file.bytes)?;
    written.sync_all()?;
    sync_directory(
        path.parent().ok_or_else(|| {
            io::Error::other("missing Claude transcript parent")
        })?,
    )
}

fn publish(
    staging: &Path,
    project: &Path,
    projects: &Path,
    archive: &Transcript,
) -> io::Result<()> {
    sync_directory(staging)?;
    match fs::rename(staging, project) {
        Ok(()) => sync_directory(projects),
        Err(error) => retry_publication(project, archive, error),
    }
}

fn retry_publication(
    project: &Path,
    archive: &Transcript,
    error: io::Error,
) -> io::Result<()> {
    if directory_exists(project)? {
        same_project(project, archive)
    } else {
        Err(error)
    }
}

fn same_project(project: &Path, archive: &Transcript) -> io::Result<()> {
    let existing = collect(project)?;
    if archive.files.len() == existing.files.len()
        && archive
            .files
            .iter()
            .all(|file| existing.files.contains(file))
    {
        Ok(())
    } else {
        Err(io::Error::other(
            "destination Claude project already contains different history",
        ))
    }
}

fn staged_path(staging: &Path, path: &str) -> io::Result<PathBuf> {
    let (file, directories) = path
        .rsplit_once('/')
        .map_or((path, ""), |(prefix, file)| (file, prefix));
    let mut parent = staging.to_path_buf();
    for component in directories.split('/').filter(|part| !part.is_empty()) {
        parent = create_directory(&parent, component)?;
    }
    Ok(parent.join(file))
}

fn remove(home: &Path, session: SessionId, ids: &[String]) -> io::Result<()> {
    validate_ids(ids).map_err(io::Error::other)?;
    directory(home)?;
    let projects = home.join("projects");
    if !directory_exists(&projects)? {
        return Ok(());
    }
    let name = session_project(session);
    let project = projects.join(&name);
    let retired = projects.join(format!(".conductor-retired-{name}"));
    retire_project(&projects, &project, &retired, ids)?;
    clear_retired(&projects, &retired)
}

fn retire_project(
    projects: &Path,
    project: &Path,
    retired: &Path,
    ids: &[String],
) -> io::Result<()> {
    if !directory_exists(project)? {
        return Ok(());
    }
    if directory_exists(retired)? {
        return Err(io::Error::other("already retiring a Claude transcript"));
    }
    check_conversations(project, ids)?;
    fs::rename(project, retired)?;
    sync_directory(projects)
}

fn check_conversations(project: &Path, ids: &[String]) -> io::Result<()> {
    for id in ids {
        let file = project.join(format!("{id}.jsonl"));
        if !fs::symlink_metadata(file)?.file_type().is_file() {
            return Err(io::Error::other(
                "Claude transcript is not a regular file",
            ));
        }
    }
    Ok(())
}

fn clear_retired(projects: &Path, retired: &Path) -> io::Result<()> {
    if directory_exists(retired)? {
        fs::remove_dir_all(retired)?;
        sync_directory(projects)?;
    }
    Ok(())
}

fn directory_exists(path: &Path) -> io::Result<bool> {
    match directory(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use proto::SessionId;
    use runtime::TranscriptFile;
    use std::fs;

    use super::{FILES, session_project};

    const CURRENT: &str = "71c93130-7464-4167-9247-173751f38179";
    const BEFORE_FORK: &str = "71c93130-7464-4167-9247-173751f38180";

    #[test]
    fn keeps_fork_histories_and_memory_in_the_moved_project()
    -> Result<(), Box<dyn std::error::Error>> {
        let sandbox = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
        let source = sandbox.path().join("source");
        let target = sandbox.path().join("target");
        fs::create_dir(&target)?;
        let session = SessionId(42);
        let ids = [String::from(CURRENT), String::from(BEFORE_FORK)];
        let project = source.join("projects").join(session_project(session));
        fs::create_dir_all(project.join("memory"))?;
        fs::write(source.join("credentials.json"), "private marker")?;
        fs::write(project.join("memory/MEMORY.md"), "Remember alpha")?;
        fs::write(source.join("projects/other"), "unrelated marker")?;
        for id in &ids {
            fs::write(
                project.join(format!("{id}.jsonl")),
                format!(
                    "{{\"type\":\"user\",\"sessionId\":\"{id}\",\"cwd\":\"/old/worktree\"}}\r\n"
                ),
            )?;
        }
        let archive = (FILES.export)(&source, session, &ids)?;
        assert_eq!(archive.files.len(), 3);
        for _ in 0..2 {
            (FILES.import)(&target, session, &ids, &archive)?;
        }
        assert_eq!((FILES.export)(&target, session, &ids)?, archive);
        assert!(!target.join("credentials.json").exists());

        let mut invalid = archive;
        invalid.files.push(TranscriptFile {
            path: String::from("memory/../../settings.json"),
            bytes: Vec::from(*b"{}"),
        });
        let refused = sandbox.path().join("refused");
        fs::create_dir(&refused)?;
        assert!((FILES.import)(&refused, session, &ids, &invalid).is_err());
        assert!(!refused.join("projects").exists());

        for _ in 0..2 {
            (FILES.remove)(&source, session, &ids)?;
        }
        assert!(!project.exists());
        assert_eq!(
            fs::read_to_string(source.join("credentials.json"))?,
            "private marker"
        );
        assert_eq!(
            fs::read_to_string(source.join("projects/other"))?,
            "unrelated marker"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn never_copies_a_link_outside_the_project()
    -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
        let source = folder.path();
        let session = SessionId(43);
        let project = source.join("projects").join(session_project(session));
        fs::create_dir_all(&project)?;
        let secret = source.join("settings.json");
        fs::write(&secret, "{}")?;
        std::os::unix::fs::symlink(
            secret,
            project.join(format!("{CURRENT}.jsonl")),
        )?;
        assert!(
            (FILES.export)(source, session, &[String::from(CURRENT)]).is_err()
        );
        Ok(())
    }
}
