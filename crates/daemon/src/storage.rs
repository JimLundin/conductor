use anyhow::Context;
use std::path::Path;

use host::log::{Epoch, Event};
use host::session::SessionId;
use link::CommandId;
use link::CommandResult;
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
            .get(key.0.as_u128())?
            .map(|value| decode(&value.value()))
            .transpose()
    }
}

fn event(write: &WriteTransaction, event: &Event) -> anyhow::Result<()> {
    let encoded = encode(event)?;
    let _ = write.open_table(EVENTS)?.insert(event.seq.0, &encoded)?;
    Ok(())
}

fn conversation(
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

pub fn store_receipt(
    write: &WriteTransaction,
    key: CommandId,
    result: &CommandResult,
) -> anyhow::Result<()> {
    let encoded = encode(result)?;
    let _ = write
        .open_table(RECEIPTS)?
        .insert(key.0.as_u128(), &encoded)?;
    Ok(())
}

pub fn write(
    write: &WriteTransaction,
    what: &host::log::Write,
) -> anyhow::Result<()> {
    match what {
        host::log::Write::Event(logged) => event(write, logged),
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
mod trace;

#[cfg(test)]
mod tests {
    use agents::ToolCallId;
    use host::log::{Event, FORMAT, HostSeq};
    use host::session::{EventKind, RequestId, SessionId};
    use link::CommandResult;

    use super::trace::Trace;
    use super::{Storage, conversation, event, store_receipt};

    #[test]
    fn every_stored_variant_keeps_its_place_in_this_format() {
        let trace = Trace::default()
            .with_text("an issue id of 8 base 32 digits", "00000000")
            .with_text("a project id of 6 base 32 digits", "000000")
            .with_text("lowercase words joined by `-`", "a")
            .of::<Event>()
            .of::<CommandResult>();
        assert!(trace.complete());
        assert_eq!(
            format!("format {FORMAT}\n{}", trace.render()),
            include_str!("storage/stored-variants.txt"),
            "a stored variant moved or was added: bump host::log::FORMAT and write the new order to storage/stored-variants.txt"
        );
    }

    #[test]
    fn committed_events_receipts_and_conversations_reopen_together()
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
                tool: agents::ToolKind::Other {
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
        store_receipt(
            &write,
            link::CommandId(uuid::Uuid::from_u128(1)),
            &CommandResult::Session(host::session::SessionResult::Accepted),
        )?;
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
            second.receipt(link::CommandId(uuid::Uuid::from_u128(1)))?,
            Some(CommandResult::Session(
                host::session::SessionResult::Accepted
            ))
        );
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
        store_receipt(
            &write,
            link::CommandId(uuid::Uuid::from_u128(1)),
            &CommandResult::Session(host::session::SessionResult::Accepted),
        )?;
        drop(write);
        drop(first);
        let second = Storage::open(&path)?;
        assert_eq!(second.events()?, Vec::new());
        assert_eq!(
            second.receipt(link::CommandId(uuid::Uuid::from_u128(1)))?,
            None
        );
        drop(second);
        let _ = std::fs::remove_file(path);
        Ok(())
    }
}
