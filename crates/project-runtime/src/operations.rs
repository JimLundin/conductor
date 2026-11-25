use crate::storage::{CACHE_BYTES, private};
use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};

use home::issue::EditId;
use home::operations::{Confirmed, Handle, Intent, Record, Refusal, Step};
use redb::{Database, ReadableDatabase, TableDefinition};

const OWNER: TableDefinition<(), [u8; 32]> = TableDefinition::new("owner/1");
const RECORDS: TableDefinition<u128, &[u8]> =
    TableDefinition::new("operations/1");
const FILE: &str = "project-operations.redb";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Refused(#[from] Refusal),
    #[error("another project operation holds the host store")]
    Busy,
    #[error("the host data directory belongs to another owner")]
    WrongHost,
    #[error("operation storage: {0}")]
    Store(String),
}

fn failed(error: impl std::fmt::Display) -> Error {
    Error::Store(error.to_string())
}

fn opening(error: redb::DatabaseError) -> Error {
    match error {
        redb::DatabaseError::DatabaseAlreadyOpen => Error::Busy,
        other => failed(other),
    }
}

#[derive(Clone)]
pub struct Operations {
    path: PathBuf,
    owner: [u8; 32],
}

impl Operations {
    pub fn bind(data: &Path, owner: [u8; 32]) -> Result<Self, Error> {
        std::fs::create_dir_all(data).map_err(failed)?;
        let path = data.join(FILE);
        let _lease = lock(&path)?;
        let database = Database::builder()
            .set_cache_size(CACHE_BYTES)
            .create_file(private(&path).map_err(failed)?)
            .map_err(opening)?;
        let write = database.begin_write().map_err(failed)?;
        {
            use redb::ReadableTable as _;
            let mut owners = write.open_table(OWNER).map_err(failed)?;
            let bound =
                owners.get(()).map_err(failed)?.map(|item| item.value());
            if bound.is_some_and(|other| other != owner) {
                return Err(Error::WrongHost);
            }
            owners.insert((), owner).map_err(failed)?;
        }
        write.open_table(RECORDS).map_err(failed)?;
        write.commit().map_err(failed)?;
        Ok(Self { path, owner })
    }

    pub fn open(data: &Path) -> Result<Self, Error> {
        let path = data.join(FILE);
        let _lease = lock(&path)?;
        let database = Database::builder()
            .set_cache_size(CACHE_BYTES)
            .open(&path)
            .map_err(opening)?;
        let owner = read_owner(&database)?;
        Ok(Self { path, owner })
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
        let _lease = lock(&self.path)?;
        let database = Database::builder()
            .set_cache_size(CACHE_BYTES)
            .open(&self.path)
            .map_err(opening)?;
        handle.local(read_owner(&database)?)?;
        read(&database, handle)
    }

    pub fn begin(
        &self,
        handle: Handle,
        intent: Intent,
        online: bool,
    ) -> Result<Attempt, Error> {
        handle.local(self.owner)?;
        let lease = lock(&self.path)?;
        let database = Database::builder()
            .set_cache_size(CACHE_BYTES)
            .open(&self.path)
            .map_err(opening)?;
        handle.local(read_owner(&database)?)?;
        let stored = read(&database, handle)?;
        let fresh = stored.is_none();
        let record = Record::resume(handle, intent, stored, online)?;
        if fresh {
            save(&database, &record)?;
        }
        Ok(Attempt {
            database,
            record,
            online,
            _lease: lease,
        })
    }
}

fn lock(path: &Path) -> Result<File, Error> {
    let lease = private(&path.with_extension("lock")).map_err(failed)?;
    lease.try_lock().map_err(|err| match err {
        TryLockError::WouldBlock => Error::Busy,
        TryLockError::Error(cause) => failed(cause),
    })?;
    Ok(lease)
}

fn read_owner(database: &impl ReadableDatabase) -> Result<[u8; 32], Error> {
    let txn = database.begin_read().map_err(failed)?;
    let table = txn.open_table(OWNER).map_err(failed)?;
    table
        .get(())
        .map_err(failed)?
        .map(|owner| owner.value())
        .ok_or(Error::WrongHost)
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
    database: Database,
    online: bool,
    _lease: File,
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

    fn replace(&mut self, record: Record) -> Result<(), Error> {
        save(&self.database, &record)?;
        self.record = record;
        Ok(())
    }
}
