//! The pinned app-server names a rollout with its stable thread UUID. A
//! reverted rollout adds a second UUID and may reference a retained prefix.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, ErrorKind, Write};
use std::path::Path;

use codex::transcript::{
    ARCHIVED, SESSIONS, calendar_component, metadata, rollout, threads,
    validate,
};
use runtime::transcript::{
    MAX_TRANSCRIPT_FILES, create_directory, directory, read_file,
    sync_directory,
};
use runtime::{
    MAX_TRANSCRIPT_BYTES, Transcript, TranscriptFile, TranscriptFiles,
};
use uuid::Uuid;

pub const FILES: TranscriptFiles = TranscriptFiles {
    export: |home, _, ids| export(home, ids).map_err(|error| error.to_string()),
    import: |home, _, ids, archive| {
        import(home, ids, archive).map_err(|error| error.to_string())
    },
    remove: |home, _, ids| remove(home, ids).map_err(|error| error.to_string()),
};

fn export(home: &Path, ids: &[String]) -> io::Result<Transcript> {
    let expected = threads(ids).map_err(io::Error::other)?;
    let archive = collect(home, &expected)?;
    validate(&archive, &expected).map_err(io::Error::other)?;
    Ok(archive)
}

struct Collection {
    archive: Transcript,
    pending: Vec<(String, usize)>,
    remaining: usize,
}

fn collect(home: &Path, expected: &BTreeSet<Uuid>) -> io::Result<Transcript> {
    directory(home)?;
    let mut contents = Collection {
        archive: Transcript { files: Vec::new() },
        pending: vec![(String::from(SESSIONS), 0), (String::from(ARCHIVED), 3)],
        remaining: MAX_TRANSCRIPT_BYTES,
    };
    while let Some((name, depth)) = contents.pending.pop() {
        contents.scan(home, expected, &name, depth)?;
    }
    contents.archive.files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(contents.archive)
}

impl Collection {
    fn scan(
        &mut self,
        home: &Path,
        expected: &BTreeSet<Uuid>,
        name: &str,
        depth: usize,
    ) -> io::Result<()> {
        if let Some(entries) = entries(&home.join(name))? {
            for entry in entries {
                self.entry(expected, name, depth, &entry?)?;
            }
        }
        Ok(())
    }

    fn entry(
        &mut self,
        expected: &BTreeSet<Uuid>,
        name: &str,
        depth: usize,
        entry: &fs::DirEntry,
    ) -> io::Result<()> {
        let Ok(component) = entry.file_name().into_string() else {
            return Ok(());
        };
        let relative = format!("{name}/{component}");
        if depth < 3 {
            self.calendar(entry, &component, relative, depth)
        } else {
            self.candidate(&entry.path(), expected, relative)
        }
    }

    fn calendar(
        &mut self,
        entry: &fs::DirEntry,
        component: &str,
        relative: String,
        depth: usize,
    ) -> io::Result<()> {
        if entry.file_type()?.is_dir() && calendar_component(component, depth) {
            self.pending.push((relative, depth + 1));
        }
        Ok(())
    }

    fn candidate(
        &mut self,
        path: &Path,
        expected: &BTreeSet<Uuid>,
        relative: String,
    ) -> io::Result<()> {
        let plain = relative.strip_suffix(".zst").unwrap_or(&relative);
        if let Some(wire) =
            rollout(plain).filter(|wire| expected.contains(&wire.thread))
        {
            self.file(path, plain != relative, relative, &wire)?;
        }
        Ok(())
    }

    fn file(
        &mut self,
        path: &Path,
        compressed: bool,
        name: String,
        wire: &codex::transcript::Rollout,
    ) -> io::Result<()> {
        if compressed {
            return Err(io::Error::other(
                "compressed Codex rollout is not supported for transfer",
            ));
        }
        if self.archive.files.len() >= MAX_TRANSCRIPT_FILES {
            return Err(io::Error::other(
                "Codex transcript file limit exceeded",
            ));
        }
        let file = read_file(path, name, &mut self.remaining)?;
        let _ = metadata(&file.bytes, wire).map_err(io::Error::other)?;
        self.archive.files.push(file);
        Ok(())
    }
}

fn entries(path: &Path) -> io::Result<Option<fs::ReadDir>> {
    match directory(path) {
        Ok(()) => fs::read_dir(path).map(Some),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn import(home: &Path, ids: &[String], archive: &Transcript) -> io::Result<()> {
    let expected = threads(ids).map_err(io::Error::other)?;
    validate(archive, &expected).map_err(io::Error::other)?;
    let existing = collect(home, &expected)?;
    if !existing
        .files
        .iter()
        .all(|file| archive.files.contains(file))
    {
        return Err(io::Error::other(
            "destination Codex home already contains different history",
        ));
    }

    let staging = archive
        .files
        .iter()
        .filter(|file| !existing.files.contains(file))
        .map(|file| stage(home, file))
        .collect::<io::Result<Vec<_>>>()?;
    for (file, path) in staging {
        file.persist_noclobber(&path).map_err(|error| error.error)?;
        sync_directory(path.parent().ok_or_else(|| {
            io::Error::other("missing Codex rollout parent")
        })?)?;
    }
    Ok(())
}

fn stage(
    home: &Path,
    file: &TranscriptFile,
) -> io::Result<(tempfile::NamedTempFile, std::path::PathBuf)> {
    let (prefix, name) = file
        .path
        .rsplit_once('/')
        .ok_or_else(|| io::Error::other("missing Codex rollout directory"))?;
    let mut parent = home.to_path_buf();
    for component in prefix.split('/') {
        parent = create_directory(&parent, component)?;
    }
    let mut staged = tempfile::NamedTempFile::new_in(&parent)?;
    staged.write_all(&file.bytes)?;
    staged.as_file().sync_all()?;
    Ok((staged, parent.join(name)))
}

fn remove(home: &Path, ids: &[String]) -> io::Result<()> {
    let expected = threads(ids).map_err(io::Error::other)?;
    let archive = collect(home, &expected)?;
    for file in archive.files {
        let path = home.join(file.path);
        remove_rollout(&path)?;
    }
    Ok(())
}

fn remove_rollout(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => {
            sync_directory(path.parent().ok_or_else(|| {
                io::Error::other("missing Codex rollout parent")
            })?)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use proto::SessionId;
    use serde_json::json;

    use super::FILES;

    const THREAD: &str = "03923c9a-736e-4edb-b619-7e37fdf54ab0";
    const RESTORED: &str = "01929c05-2e30-73f2-9fed-2473ed55d6e5";
    const UNRELATED: &str = "3c418804-d2c1-45d3-8ca4-dc03831d7d56";

    fn put(home: &Path, path: &str, row: &str) -> std::io::Result<()> {
        let file = home.join(path);
        let parent = file
            .parent()
            .ok_or_else(|| std::io::Error::other("missing test parent"))?;
        fs::create_dir_all(parent)?;
        fs::write(file, row)
    }

    #[test]
    fn a_reverted_thread_moves_with_its_retained_prefix()
    -> Result<(), Box<dyn std::error::Error>> {
        let sandbox = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
        let source = sandbox.path().join("source");
        let target = sandbox.path().join("target");
        fs::create_dir(&target)?;
        let session = SessionId(44);
        let ids = [String::from(THREAD)];
        let original = format!(
            "archived_sessions/rollout-2026-11-10T17-25-00-{THREAD}.jsonl"
        );
        let active = format!(
            "sessions/2026/11/11/rollout-2026-11-11T08-45-14-{THREAD}_{RESTORED}.jsonl"
        );
        let other = format!(
            "sessions/2026/11/11/rollout-2026-11-11T08-45-14-{UNRELATED}.jsonl"
        );
        let prefix = format!(
            "{}\n",
            json!({
                "type": "session_meta",
                "payload": { "id": THREAD, "cwd": "/old/worktree" }
            })
        );
        let changed = format!(
            "{}\n",
            json!({
                "type": "session_meta",
                "payload": { "id": THREAD, "cwd": "/old/worktree", "history_base": {
                    "thread_id": THREAD, "end_ordinal_exclusive": 1, "end_byte_offset": prefix.len()
                }}
            })
        );
        put(&source, &original, &prefix)?;
        put(&source, &active, &changed)?;
        put(
            &source,
            &other,
            &format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{UNRELATED}\"}}}}\n"
            ),
        )?;
        fs::write(source.join("auth.json"), "private marker")?;
        fs::write(source.join("session_index.jsonl"), "index marker")?;

        let archive = (FILES.export)(&source, session, &ids)?;
        assert_eq!(archive.files.len(), 2);
        let mut missing_prefix = archive.clone();
        missing_prefix.files.retain(|file| file.path != original);
        assert!(
            (FILES.import)(&target, session, &ids, &missing_prefix).is_err()
        );
        assert!(!target.join("sessions").exists());

        put(&target, &original, &prefix)?;
        for _ in 0..2 {
            (FILES.import)(&target, session, &ids, &archive)?;
        }
        assert_eq!((FILES.export)(&target, session, &ids)?, archive);
        assert!(!target.join("session_index.jsonl").exists());
        assert!(!target.join("auth.json").exists());
        assert!(!target.join(&other).exists());

        for _ in 0..2 {
            (FILES.remove)(&source, session, &ids)?;
        }
        assert!(!source.join(&active).exists());
        assert!(source.join(&other).exists());
        assert_eq!(
            fs::read_to_string(source.join("auth.json"))?,
            "private marker"
        );
        assert_eq!(
            fs::read_to_string(source.join("session_index.jsonl"))?,
            "index marker"
        );
        Ok(())
    }

    #[test]
    fn a_filename_never_overrides_a_different_rollout_owner()
    -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
        let path = format!(
            "sessions/2026/11/11/rollout-2026-11-11T08-45-14-{THREAD}.jsonl"
        );
        put(
            folder.path(),
            &path,
            &format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{UNRELATED}\"}}}}\n"
            ),
        )?;
        let ids = [String::from(THREAD)];
        let session = SessionId(45);
        assert!((FILES.export)(folder.path(), session, &ids).is_err());
        assert!((FILES.remove)(folder.path(), session, &ids).is_err());
        assert!(folder.path().join(path).is_file());
        Ok(())
    }
}
