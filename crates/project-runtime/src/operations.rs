use crate::storage::{CACHE_BYTES, private};
use std::fmt::Write as _;
use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};

use home::issue::EditId;
use home::issue::{Comment, NativeIssue, Opened, Relating};
use home::operations::{
    Advance, Confirmed, Handle, Intent, Next, Opening, Progress, Record,
    Refusal, Relate, Step,
};
use redb::{Database, ReadableDatabase, TableDefinition};

const OWNER: TableDefinition<(), [u8; 32]> = TableDefinition::new("owner/1");
const RECORDS: TableDefinition<u128, &[u8]> =
    TableDefinition::new("operations/3");
const FILE: &str = "project-operations.redb";
const IDENTITY: &str = "home-identity";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Refused(#[from] Refusal),
    #[error("a write on this issue is already in progress")]
    InProgress,
    #[error("operation storage: {0}")]
    Store(String),
}

fn failed(error: impl std::fmt::Display) -> Error {
    Error::Store(error.to_string())
}

#[derive(Clone)]
pub struct Operations {
    path: PathBuf,
    owner: [u8; 32],
}

impl Operations {
    pub fn adopt(
        data: &Path,
        fresh: impl FnOnce() -> Result<[u8; 32], Error>,
    ) -> Result<Self, Error> {
        std::fs::create_dir_all(data).map_err(failed)?;
        let path = data.join(FILE);
        let _lease = lock(&path)?;
        let database = Database::builder()
            .set_cache_size(CACHE_BYTES)
            .create_file(private(&path).map_err(failed)?)
            .map_err(failed)?;
        let write = database.begin_write().map_err(failed)?;
        let owner = {
            use redb::ReadableTable as _;
            let mut owners = write.open_table(OWNER).map_err(failed)?;
            let bound =
                owners.get(()).map_err(failed)?.map(|item| item.value());
            let owner = bound.map_or_else(fresh, Ok)?;
            owners.insert((), owner).map_err(failed)?;
            owner
        };
        write.open_table(RECORDS).map_err(failed)?;
        write.commit().map_err(failed)?;
        Ok(Self { path, owner })
    }

    pub fn home(data: &Path) -> Result<Self, Error> {
        Self::adopt(data, || identity(data))
    }

    pub fn handle(&self) -> Result<Handle, Error> {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).map_err(failed)?;
        Ok(Handle {
            owner: self.owner,
            id: EditId(u128::from_le_bytes(bytes)),
        })
    }

    pub fn status(&self, handle: Handle) -> Result<Option<Record>, Error> {
        handle.local(self.owner)?;
        change(&self.path, |database| {
            handle.local(read_owner(database)?)?;
            read(database, handle)
        })
    }

    pub fn unsettled(&self) -> Result<Vec<Record>, Error> {
        change(&self.path, |database| {
            use redb::ReadableTable as _;
            let txn = database.begin_read().map_err(failed)?;
            let records = txn.open_table(RECORDS).map_err(failed)?;
            let mut open = Vec::new();
            for item in records.iter().map_err(failed)? {
                let (_, value) = item.map_err(failed)?;
                let record: Record =
                    postcard::from_bytes(value.value()).map_err(failed)?;
                if *record.progress() != Progress::Completed {
                    open.push(record);
                }
            }
            Ok(open)
        })
    }

    pub fn begin(
        &self,
        handle: Handle,
        intent: Intent,
        online: bool,
    ) -> Result<Attempt, Error> {
        handle.local(self.owner)?;
        let claim = claim(&self.path, &subject(handle, &intent)?)?;
        let record = change(&self.path, |database| {
            handle.local(read_owner(database)?)?;
            let stored = read(database, handle)?;
            let fresh = stored.is_none();
            let record = Record::resume(handle, intent, stored, online)?;
            if fresh {
                save(database, &record)?;
            }
            Ok(record)
        })?;
        Ok(Attempt {
            path: self.path.clone(),
            record,
            online,
            _claim: claim,
        })
    }
}

fn identity(data: &Path) -> Result<[u8; 32], Error> {
    use std::io::{Read as _, Write as _};
    let mut file = private(&data.join(IDENTITY)).map_err(failed)?;
    let mut kept = Vec::new();
    file.read_to_end(&mut kept).map_err(failed)?;
    if let Ok(owner) = <[u8; 32]>::try_from(kept.as_slice()) {
        return Ok(owner);
    }
    if !kept.is_empty() {
        return Err(Error::Store(format!("{IDENTITY} is not 32 bytes")));
    }
    let mut owner = [0; 32];
    getrandom::fill(&mut owner).map_err(failed)?;
    file.write_all(&owner).map_err(failed)?;
    file.sync_all().map_err(failed)?;
    Ok(owner)
}

fn lock(path: &Path) -> Result<File, Error> {
    let lease = private(&path.with_extension("lock")).map_err(failed)?;
    lease.lock().map_err(failed)?;
    Ok(lease)
}

fn change<T>(
    path: &Path,
    apply: impl FnOnce(&Database) -> Result<T, Error>,
) -> Result<T, Error> {
    let _lease = lock(path)?;
    let database = Database::builder()
        .set_cache_size(CACHE_BYTES)
        .open(path)
        .map_err(failed)?;
    apply(&database)
}

struct Claim {
    path: PathBuf,
    _file: File,
}

fn subject(handle: Handle, intent: &Intent) -> Result<String, Error> {
    let (kind, bytes) = match intent.issue() {
        Some(issue) => {
            ("issue", postcard::to_stdvec(&(issue.project, issue.id)))
        }
        None => ("opening", postcard::to_stdvec(&handle.id)),
    };
    let mut named = format!("{kind}-");
    for byte in bytes.map_err(failed)? {
        write!(named, "{byte:02x}").map_err(failed)?;
    }
    Ok(named)
}

fn claim(store: &Path, subject: &str) -> Result<Claim, Error> {
    use std::os::unix::fs::MetadataExt as _;
    let claims = store.with_extension("claims");
    std::fs::create_dir_all(&claims).map_err(failed)?;
    let path = claims.join(subject);
    loop {
        let file = private(&path).map_err(failed)?;
        file.try_lock().map_err(|err| match err {
            TryLockError::WouldBlock => Error::InProgress,
            TryLockError::Error(cause) => failed(cause),
        })?;
        let held = file.metadata().map_err(failed)?;
        let current = std::fs::metadata(&path).ok();
        if current.is_some_and(|named| {
            (named.dev(), named.ino()) == (held.dev(), held.ino())
        }) {
            return Ok(Claim { path, _file: file });
        }
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).ok();
    }
}
fn read_owner(database: &impl ReadableDatabase) -> Result<[u8; 32], Error> {
    let txn = database.begin_read().map_err(failed)?;
    let table = txn.open_table(OWNER).map_err(failed)?;
    table
        .get(())
        .map_err(failed)?
        .map(|owner| owner.value())
        .ok_or_else(|| Error::Store(String::from("the store has no owner")))
}

fn read(
    database: &impl ReadableDatabase,
    handle: Handle,
) -> Result<Option<Record>, Error> {
    let txn = database.begin_read().map_err(failed)?;
    let records = txn.open_table(RECORDS).map_err(failed)?;
    records
        .get(handle.id.0)
        .map_err(failed)?
        .map(|value| postcard::from_bytes(value.value()))
        .transpose()
        .map_err(failed)
}

fn save(database: &Database, record: &Record) -> Result<(), Error> {
    let bytes = postcard::to_stdvec(record).map_err(failed)?;
    let write = database.begin_write().map_err(failed)?;
    write
        .open_table(RECORDS)
        .map_err(failed)?
        .insert(record.handle.id.0, bytes.as_slice())
        .map_err(failed)?;
    write.commit().map_err(failed)
}

pub struct Attempt {
    path: PathBuf,
    online: bool,
    _claim: Claim,
    record: Record,
}

impl Attempt {
    #[must_use]
    pub const fn record(&self) -> &Record {
        &self.record
    }

    pub fn dispatch(
        &mut self,
        step: Step,
        observed: &str,
    ) -> Result<(), Error> {
        self.replace(self.record.dispatch(step, observed, self.online)?)
    }

    pub fn confirm(
        &mut self,
        evidence: Confirmed,
        complete: bool,
    ) -> Result<(), Error> {
        self.replace(self.record.confirm(evidence, complete)?)
    }

    pub fn next(
        &mut self,
        observed: &[Comment],
        resuming: bool,
    ) -> Result<Next, Error> {
        let (record, next) = self.record.next(observed, resuming)?;
        self.replace(record)?;
        Ok(next)
    }

    pub fn relate(
        &mut self,
        plan: &Relating,
        resuming: bool,
    ) -> Result<Relate, Error> {
        let (record, next) = self.record.relate(plan, resuming)?;
        self.replace(record)?;
        Ok(next)
    }

    pub fn opened(
        &mut self,
        found: &[Opened],
        resuming: bool,
    ) -> Result<Opening, Error> {
        let (record, opening) = self.record.opened(found, resuming)?;
        self.replace(record)?;
        Ok(opening)
    }

    pub fn advance(
        &mut self,
        observed: &NativeIssue,
        resuming: bool,
    ) -> Result<Advance, Error> {
        let (record, advance) = self.record.advance(observed, resuming)?;
        self.replace(record)?;
        Ok(advance)
    }

    pub fn reject(&mut self) -> Result<(), Error> {
        self.replace(self.record.reject()?)
    }

    fn replace(&mut self, record: Record) -> Result<(), Error> {
        change(&self.path, |database| save(database, &record))?;
        self.record = record;
        Ok(())
    }
}
