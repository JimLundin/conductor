use super::CacheError;
use crate::storage::{CACHE_BYTES, private};
use home::ProjectId;
use home::backend::Selection;
use home::flow::Flow;
use home::issue::cache::Collection;
use redb::{
    Database, ReadableDatabase, ReadableTable, TableDefinition, TableError,
};
use std::fs::File;
use std::path::{Path, PathBuf};

const COLLECTIONS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("issue-collections/3");

const GENERATION: TableDefinition<(), u64> =
    TableDefinition::new("refresh-generation/1");

pub(super) struct Scope {
    path: PathBuf,
    key: Vec<u8>,
}

fn failed(error: impl std::fmt::Display) -> CacheError {
    CacheError::Store(error.to_string())
}

struct Storage {
    database: Database,
    _lease: File,
}

impl Scope {
    pub(super) fn new(
        data: &Path,
        project: ProjectId,
        selection: &Selection,
        flow: &Flow,
    ) -> Result<Self, CacheError> {
        let key =
            postcard::to_stdvec(&(project, selection, flow)).map_err(failed)?;
        std::fs::create_dir_all(data).map_err(failed)?;
        Ok(Self {
            path: data.join("issue-cache.redb"),
            key,
        })
    }

    fn open(&self) -> Result<Storage, CacheError> {
        let lease =
            private(&self.path.with_extension("lock")).map_err(failed)?;
        lease.lock().map_err(failed)?;
        let database = Database::builder()
            .set_cache_size(CACHE_BYTES)
            .create_file(private(&self.path).map_err(failed)?)
            .map_err(failed)?;
        Ok(Storage {
            database,
            _lease: lease,
        })
    }

    pub(super) fn load(&self) -> Result<Collection, CacheError> {
        let storage = self.open()?;
        let txn = storage.database.begin_read().map_err(failed)?;
        let table = match txn.open_table(COLLECTIONS) {
            Ok(table) => table,
            Err(TableError::TableDoesNotExist(_)) => {
                return Ok(Collection::default());
            }
            Err(error) => return Err(failed(error)),
        };
        table
            .get(self.key.as_slice())
            .map_err(failed)?
            .map(|bytes| postcard::from_bytes(bytes.value()))
            .transpose()
            .map_err(failed)
            .map(Option::unwrap_or_default)
    }

    pub(super) fn begin(&self) -> Result<u64, CacheError> {
        let storage = self.open()?;
        let txn = storage.database.begin_write().map_err(failed)?;
        let mut counter = txn.open_table(GENERATION).map_err(failed)?;
        let previous = counter
            .get(())
            .map_err(failed)?
            .map_or(0, |stored| stored.value());
        let next = previous.checked_add(1).ok_or_else(|| {
            CacheError::Store(String::from(
                "issue refresh generation exhausted",
            ))
        })?;
        counter.insert((), next).map_err(failed)?;
        drop(counter);
        txn.commit().map_err(failed)?;
        Ok(next)
    }

    pub(super) fn save<T>(
        &self,
        change: impl FnOnce(&mut Collection) -> T,
    ) -> Result<(Collection, T), CacheError> {
        let storage = self.open()?;
        let txn = storage.database.begin_write().map_err(failed)?;
        let mut table = txn.open_table(COLLECTIONS).map_err(failed)?;
        let old = table.get(self.key.as_slice()).map_err(failed)?;
        let mut records = old
            .map(|b| postcard::from_bytes(b.value()))
            .transpose()
            .map_err(failed)?
            .unwrap_or_default();
        let result = change(&mut records);
        let bytes = postcard::to_stdvec(&records).map_err(failed)?;
        table
            .insert(self.key.as_slice(), bytes.as_slice())
            .map_err(failed)?;
        drop(table);
        txn.commit().map_err(failed)?;
        Ok((records, result))
    }
}
