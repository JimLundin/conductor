use std::fmt::Display;
use std::fs::File;
use std::path::{Path, PathBuf};

use home::ProjectId;
use home::backend::Selection;
use home::canvas::CanvasKey;
use home::issue::IssueComment;
use home::operations::Handle;
use project_runtime::storage::{CACHE_BYTES, private};
use redb::{
    Database, ReadableDatabase, ReadableTable, TableDefinition, TableError,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

const COMMENTS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("canvas-comments/1");
const CREATES: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("canvas-creates/1");
const FILE: &str = "github-canvases.redb";

pub struct CanvasCache {
    path: PathBuf,
    scope: Vec<u8>,
}

struct Storage {
    database: Database,
    _lease: File,
}

fn failed(error: impl Display) -> String {
    format!("GitHub's canvas cache: {error}")
}

impl CanvasCache {
    pub fn open(
        data: &Path,
        project: ProjectId,
        selection: &Selection,
    ) -> Result<Self, String> {
        std::fs::create_dir_all(data).map_err(failed)?;
        Ok(Self {
            path: data.join(FILE),
            scope: postcard::to_stdvec(&(project, selection))
                .map_err(failed)?,
        })
    }

    pub fn comments(&self) -> Result<Vec<IssueComment>, String> {
        Ok(self.read(COMMENTS, &self.scope)?.unwrap_or_default())
    }

    pub fn refreshed(&self, comments: &[IssueComment]) -> Result<(), String> {
        self.change(COMMENTS, &self.scope, |kept| {
            *kept = Some(comments.to_vec());
        })
    }

    pub fn posted(&self, posted: IssueComment) -> Result<(), String> {
        self.change(COMMENTS, &self.scope, |kept: &mut Option<Vec<_>>| {
            let comments = kept.get_or_insert_default();
            match comments.iter_mut().find(|held: &&mut IssueComment| {
                held.comment.id == posted.comment.id
            }) {
                Some(held) => *held = posted,
                None => comments.push(posted),
            }
        })
    }

    pub fn creating(&self, key: &CanvasKey) -> Result<Option<Handle>, String> {
        self.read(CREATES, &self.create(key)?)
    }

    pub fn create_with(
        &self,
        key: &CanvasKey,
        handle: Handle,
    ) -> Result<Handle, String> {
        let mut kept = handle;
        self.change(CREATES, &self.create(key)?, |pending| {
            kept = *pending.get_or_insert(handle);
        })?;
        Ok(kept)
    }

    pub fn created(
        &self,
        key: &CanvasKey,
        handle: Handle,
    ) -> Result<(), String> {
        self.change(CREATES, &self.create(key)?, |pending| {
            if *pending == Some(handle) {
                *pending = None;
            }
        })
    }

    fn create(&self, key: &CanvasKey) -> Result<Vec<u8>, String> {
        postcard::to_stdvec(&(&self.scope, key)).map_err(failed)
    }

    fn storage(&self) -> Result<Storage, String> {
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

    fn read<T: DeserializeOwned>(
        &self,
        table: TableDefinition<&[u8], &[u8]>,
        key: &[u8],
    ) -> Result<Option<T>, String> {
        let storage = self.storage()?;
        let txn = storage.database.begin_read().map_err(failed)?;
        let opened = match txn.open_table(table) {
            Ok(opened) => opened,
            Err(TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => return Err(failed(error)),
        };
        opened
            .get(key)
            .map_err(failed)?
            .map(|bytes| postcard::from_bytes(bytes.value()))
            .transpose()
            .map_err(failed)
    }

    fn change<T: Serialize + DeserializeOwned>(
        &self,
        table: TableDefinition<&[u8], &[u8]>,
        key: &[u8],
        apply: impl FnOnce(&mut Option<T>),
    ) -> Result<(), String> {
        let storage = self.storage()?;
        let txn = storage.database.begin_write().map_err(failed)?;
        {
            let mut opened = txn.open_table(table).map_err(failed)?;
            let mut value = opened
                .get(key)
                .map_err(failed)?
                .map(|bytes| postcard::from_bytes(bytes.value()))
                .transpose()
                .map_err(failed)?;
            apply(&mut value);
            match value {
                Some(kept) => {
                    let bytes = postcard::to_stdvec(&kept).map_err(failed)?;
                    opened.insert(key, bytes.as_slice()).map_err(failed)?;
                }
                None => {
                    opened.remove(key).map_err(failed)?;
                }
            }
        }
        txn.commit().map_err(failed)
    }
}
