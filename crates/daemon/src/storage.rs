//! Typed redb tables with postcard values. The log actor owns all writes.
use anyhow::Context;
use std::path::Path;

use proto::{CommandId, CommandResult, Epoch, Event, EventKind, SessionId};
use redb::{
    Database, ReadableDatabase, ReadableTable, TableDefinition,
    WriteTransaction,
};
use serde::{Serialize, de::DeserializeOwned};

const EVENTS: TableDefinition<u64, Vec<u8>> = TableDefinition::new("events");
const META: TableDefinition<&str, Vec<u8>> = TableDefinition::new("metadata");
const RECEIPTS: TableDefinition<u128, Vec<u8>> =
    TableDefinition::new("command_receipts");
const CONVERSATIONS: TableDefinition<u128, Vec<u8>> =
    TableDefinition::new("conversations");
const REQUESTS: TableDefinition<&str, Vec<u8>> =
    TableDefinition::new("pending_requests");

pub struct Storage {
    database: Database,
}

impl Storage {
    pub(crate) fn open(path: &Path) -> anyhow::Result<Self> {
        let database = Database::create(path)?;
        let write = database.begin_write()?;
        let _ = write.open_table(EVENTS)?;
        let _ = write.open_table(RECEIPTS)?;
        let _ = write.open_table(CONVERSATIONS)?;
        let _ = write.open_table(REQUESTS)?;
        {
            let mut meta = write.open_table(META)?;
            let created = meta.get("epoch")?.is_none();
            let format = meta
                .get("format")?
                .map(|value| decode::<u32>(&value.value()))
                .transpose()?;
            let fresh = host::log::opens(created, format)
                .map_err(anyhow::Error::msg)
                .with_context(|| path.display().to_string())?;
            if fresh {
                let _ =
                    meta.insert("epoch", &encode(&crate::random_epoch()?)?)?;
                let _ = meta.insert("format", &encode(&host::log::FORMAT)?)?;
            }
        }
        write.commit()?;
        Ok(Self { database })
    }

    pub(crate) fn epoch(&self) -> anyhow::Result<Epoch> {
        let read = self.database.begin_read()?;
        let table = read.open_table(META)?;
        let value = table.get("epoch")?.context("missing database epoch")?;
        decode(&value.value())
    }

    pub(crate) fn events(&self) -> anyhow::Result<Vec<Event>> {
        let read = self.database.begin_read()?;
        let table = read.open_table(EVENTS)?;
        table
            .iter()?
            .map(|row| {
                let (_, value) = row?;
                decode(&value.value())
            })
            .collect()
    }

    pub(crate) fn conversation(
        &self,
        session: SessionId,
    ) -> anyhow::Result<Option<String>> {
        let read = self.database.begin_read()?;
        let table = read.open_table(CONVERSATIONS)?;
        table
            .get(session.0)?
            .map(|value| decode(&value.value()))
            .transpose()
    }

    pub(crate) fn begin(&self) -> anyhow::Result<WriteTransaction> {
        Ok(self.database.begin_write()?)
    }

    pub(crate) fn receipt(
        &self,
        key: CommandId,
    ) -> anyhow::Result<Option<CommandResult>> {
        let read = self.database.begin_read()?;
        let table = read.open_table(RECEIPTS)?;
        table
            .get(key.0)?
            .map(|value| decode(&value.value()))
            .transpose()
    }
}

pub fn event(write: &WriteTransaction, event: &Event) -> anyhow::Result<()> {
    let encoded = encode(event)?;
    let _ = write.open_table(EVENTS)?.insert(event.seq.0, &encoded)?;
    match &event.kind {
        EventKind::RequestOpened { request, .. }
        | EventKind::QuestionAsked { request, .. }
        | EventKind::Subagent {
            event:
                proto::SubagentEvent::RequestOpened { request, .. }
                | proto::SubagentEvent::QuestionAsked { request, .. },
            ..
        } => {
            let key = format!("{}:{}", event.session.0, request.0);
            let _ =
                write.open_table(REQUESTS)?.insert(key.as_str(), &encoded)?;
        }
        EventKind::RequestResolved { request, .. } => {
            let key = format!("{}:{}", event.session.0, request.0);
            let _ = write.open_table(REQUESTS)?.remove(key.as_str())?;
        }
        EventKind::SessionClosed { .. } => {
            let prefix = format!("{}:", event.session.0);
            let mut requests = write.open_table(REQUESTS)?;
            let keys = requests
                .iter()?
                .map(|row| row.map(|(key, _)| key.value().to_owned()))
                .collect::<Result<Vec<String>, redb::StorageError>>()?;
            let closing = keys
                .into_iter()
                .filter(|key| key.starts_with(&prefix))
                .collect::<Vec<String>>();
            for key in closing {
                let _ = requests.remove(key.as_str())?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn conversation(
    write: &WriteTransaction,
    session: SessionId,
    id: &str,
) -> anyhow::Result<()> {
    let encoded = encode(&id.to_owned())?;
    let _ = write
        .open_table(CONVERSATIONS)?
        .insert(session.0, &encoded)?;
    Ok(())
}

pub fn receipt(
    write: &WriteTransaction,
    key: CommandId,
    result: &CommandResult,
) -> anyhow::Result<()> {
    let encoded = encode(result)?;
    let _ = write.open_table(RECEIPTS)?.insert(key.0, &encoded)?;
    Ok(())
}

/// Performs one of the log's writes in `write`.
pub fn write(
    write: &WriteTransaction,
    what: &host::log::Write,
) -> anyhow::Result<()> {
    match what {
        host::log::Write::Event(logged) => event(write, logged),
        host::log::Write::Receipt(key, result) => receipt(write, *key, result),
        host::log::Write::Conversation(session, id) => {
            conversation(write, *session, id)
        }
    }
}

fn encode<T: Serialize>(value: &T) -> anyhow::Result<Vec<u8>> {
    Ok(postcard::to_stdvec(value)?)
}

fn decode<T: DeserializeOwned>(value: &[u8]) -> anyhow::Result<T> {
    Ok(postcard::from_bytes(value)?)
}

#[cfg(test)]
mod tests {
    use proto::{
        CommandResult, Event, EventKind, HostSeq, RequestId, Resolution,
        SessionId, ToolCallId,
    };
    use redb::{ReadableDatabase, ReadableTableMetadata};

    use super::{REQUESTS, Storage, conversation, event, receipt};

    #[test]
    fn committed_events_receipts_conversations_and_requests_reopen_together()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = std::env::temp_dir()
            .join(format!("conductor-durable-{}.redb", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let first = Storage::open(&path)?;
        let epoch = first.epoch()?;
        let opened = Event {
            seq: HostSeq(1),
            session: SessionId(7),
            kind: EventKind::RequestOpened {
                request: RequestId(1),
                call: ToolCallId(String::from("tool")),
                tool: proto::ToolKind::Other {
                    name: String::from("test"),
                    detail: String::new(),
                },
                reason: None,
                for_session: None,
            },
        };
        let write = first.begin()?;
        event(&write, &opened)?;
        conversation(&write, SessionId(7), "native-conversation")?;
        receipt(&write, proto::CommandId(1), &CommandResult::Accepted)?;
        write.commit()?;
        drop(first);
        let second = Storage::open(&path)?;
        assert_eq!(second.epoch()?, epoch);
        assert_eq!(second.events()?, std::slice::from_ref(&opened));
        assert_eq!(
            second.conversation(SessionId(7))?,
            Some(String::from("native-conversation"))
        );
        assert_eq!(
            second.receipt(proto::CommandId(1))?,
            Some(CommandResult::Accepted)
        );
        let read = second.database.begin_read()?;
        assert_eq!(read.open_table(REQUESTS)?.len()?, 1);
        drop(read);
        let write = second.begin()?;
        event(
            &write,
            &Event {
                seq: HostSeq(2),
                session: SessionId(7),
                kind: EventKind::RequestResolved {
                    request: RequestId(1),
                    resolution: Resolution::Withdrawn,
                },
            },
        )?;
        write.commit()?;
        let read = second.database.begin_read()?;
        assert_eq!(read.open_table(REQUESTS)?.len()?, 0);
        drop(read);
        let write = second.begin()?;
        event(
            &write,
            &Event {
                seq: HostSeq(3),
                ..opened.clone()
            },
        )?;
        event(
            &write,
            &Event {
                seq: HostSeq(4),
                session: SessionId(70),
                ..opened
            },
        )?;
        event(
            &write,
            &Event {
                seq: HostSeq(5),
                session: SessionId(7),
                kind: EventKind::SessionClosed {
                    reason: proto::CloseReason::Stopped {
                        reason: String::from("stopped"),
                    },
                },
            },
        )?;
        write.commit()?;
        let read = second.database.begin_read()?;
        assert_eq!(read.open_table(REQUESTS)?.len()?, 1);
        drop(read);
        drop(second);
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn an_uncommitted_command_leaves_neither_effects_nor_receipt()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = std::env::temp_dir()
            .join(format!("conductor-rollback-{}.redb", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let first = Storage::open(&path)?;
        let write = first.begin()?;
        event(
            &write,
            &Event {
                seq: HostSeq(1),
                session: SessionId(1),
                kind: EventKind::SessionReady,
            },
        )?;
        receipt(&write, proto::CommandId(1), &CommandResult::Accepted)?;
        drop(write);
        drop(first);
        let second = Storage::open(&path)?;
        assert_eq!(second.events()?, Vec::new());
        assert_eq!(second.receipt(proto::CommandId(1))?, None);
        drop(second);
        let _ = std::fs::remove_file(path);
        Ok(())
    }
}
