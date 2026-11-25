use home::IssueId;
use home::issue::ownership::{Owner, Reservation, reserve};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use vcs::Repository;

pub(super) struct Locked {
    _lock: File,
    path: PathBuf,
    repository: Repository,
    recorded: Vec<Owner>,
}

pub(super) async fn lock(directory: &Path) -> Result<Locked, String> {
    let owned = directory.to_owned();
    tokio::task::spawn_blocking(move || open(&owned))
        .await
        .map_err(|error| error.to_string())?
}

fn open(directory: &Path) -> Result<Locked, String> {
    let canonical =
        std::fs::canonicalize(directory).map_err(|error| error.to_string())?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(canonical.join("conductor-issue-owners.lock"))
        .map_err(|error| error.to_string())?;
    lock.lock().map_err(|error| error.to_string())?;
    let path = canonical.join("conductor-issue-owners.state");
    let recorded = read(&path)?;
    Ok(Locked {
        _lock: lock,
        path,
        repository: Repository(canonical.to_string_lossy().into_owned()),
        recorded,
    })
}

fn read(path: &Path) -> Result<Vec<Owner>, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(error) => return Err(error.to_string()),
    };
    let encoded = bytes.strip_prefix(b"issue-owners/1\n").ok_or_else(|| {
        String::from(
            "the issue branch ownership record has an unsupported format",
        )
    })?;
    postcard::from_bytes(encoded).map_err(|error| error.to_string())
}

impl Locked {
    pub(super) fn reserve(
        &mut self,
        issue: &IssueId,
        branch: &str,
        exists: bool,
    ) -> Result<(), String> {
        let reservation = reserve(
            &self.repository,
            issue,
            branch,
            exists,
            &self.recorded,
        )
        .map_err(|conflict| {
            format!(
                "issue branch ownership requires reconciliation: {conflict:?}"
            )
        })?;
        reservation.requested(branch)?;
        if let Reservation::Record(owner) = reservation {
            self.recorded.push(owner);
            super::save(
                &self.path,
                b"issue-owners/1\n",
                &self.recorded,
                "issue branch ownership",
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_owner_record_is_kept_before_the_branch_exists()
    -> Result<(), String> {
        let directory =
            tempfile::tempdir().map_err(|error| error.to_string())?;
        let issue = IssueId::new("tracker-one", "rest-42")
            .ok_or_else(|| String::from("fixture identity"))?;
        let mut held = lock(directory.path()).await?;
        held.reserve(&issue, "refs/heads/kept", false)?;
        drop(held);
        let mut reopened = lock(directory.path()).await?;
        reopened.reserve(&issue, "refs/heads/kept", true)?;
        let other = IssueId::new("tracker-two", issue.key())
            .ok_or_else(|| String::from("fixture identity"))?;
        assert!(reopened.reserve(&other, "refs/heads/kept", true).is_err());
        assert!(
            reopened
                .reserve(&issue, "refs/heads/renamed", false)
                .is_err()
        );
        assert_eq!(
            read(&directory.path().join("conductor-issue-owners.state"))?.len(),
            1
        );
        Ok(())
    }
}
