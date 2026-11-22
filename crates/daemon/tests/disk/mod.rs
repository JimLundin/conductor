use std::path::Path;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use proto::{
    CommandId, CommandResult, Epoch, Event, EventKind, HostSeq, Roster,
    SessionId,
};
use redb::{Database, ReadableTable, ReadableTableMetadata, TableDefinition};

use crate::Fallible;

const HEADER: u8 = 7;
const PURPOSE: &[u8] = b"conductor credentials 1";

pub fn seed_roster(data: &Path, roster: &Roster) -> Fallible {
    let mut key = [0; 32];
    let mut nonce = [0; 24];
    getrandom::fill(&mut key)?;
    getrandom::fill(&mut nonce)?;
    let plain = postcard::to_stdvec(&(
        roster,
        std::collections::BTreeSet::<CommandId>::new(),
    ))?;
    let cipher = XChaCha20Poly1305::new(&key.into());
    let sealed = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: &plain,
                aad: PURPOSE,
            },
        )
        .map_err(|_| "cannot seal test roster")?;
    std::fs::write(
        data.join("store.key"),
        data_encoding::HEXLOWER.encode(&key),
    )?;
    std::fs::write(
        data.join("credentials"),
        [&[HEADER], nonce.as_slice(), &sealed].concat(),
    )?;
    Ok(())
}

pub fn roster(data: &Path) -> Fallible<Roster> {
    let key: [u8; 32] = data_encoding::HEXLOWER
        .decode(&std::fs::read(data.join("store.key"))?)?
        .try_into()
        .map_err(|_| "invalid test store key")?;
    let file = std::fs::read(data.join("credentials"))?;
    let (header, body) = file.split_first().ok_or("missing roster header")?;
    let (nonce, encrypted) = body
        .split_at_checked(24)
        .ok_or("not a sealed test roster")?;
    let cipher = XChaCha20Poly1305::new(&key.into());
    let plain = cipher
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: encrypted,
                aad: PURPOSE,
            },
        )
        .map_err(|_| "cannot open test roster")?;
    match *header {
        HEADER => Ok(postcard::take_from_bytes(&plain)?.0),
        _ => Err("unknown test roster format".into()),
    }
}

pub fn receipt_count(data: &Path) -> Fallible<u64> {
    use redb::ReadableDatabase as _;

    let database = Database::open(data.join("conductor.redb"))?;
    let read = database.begin_read()?;
    let receipts = read.open_table(TableDefinition::<u128, Vec<u8>>::new(
        "command_receipts",
    ))?;
    Ok(receipts.len()?)
}

pub fn remove_receipt(data: &Path, id: CommandId) -> Fallible<CommandResult> {
    let database = Database::open(data.join("conductor.redb"))?;
    let write = database.begin_write()?;
    let result = {
        let mut receipts = write.open_table(
            TableDefinition::<u128, Vec<u8>>::new("command_receipts"),
        )?;
        let removed =
            receipts.remove(id.0)?.ok_or("expected a command receipt")?;
        postcard::from_bytes(&removed.value())?
    };
    write.commit()?;
    Ok(result)
}

/// The wire cannot create pre-worktree history or records from an interrupted write.
pub fn append(
    data: &Path,
    session: SessionId,
    kinds: impl IntoIterator<Item = EventKind>,
) -> Fallible {
    let database = Database::create(data.join("conductor.redb"))?;
    let write = database.begin_write()?;
    {
        let mut meta = write
            .open_table(TableDefinition::<&str, Vec<u8>>::new("metadata"))?;
        if meta.is_empty()? {
            let _ = meta.insert("epoch", postcard::to_stdvec(&Epoch(11))?)?;
            let _ = meta
                .insert("format", postcard::to_stdvec(&host::log::FORMAT)?)?;
        }
        let mut table =
            write.open_table(TableDefinition::<u64, Vec<u8>>::new("events"))?;
        let mut seq = HostSeq(table.len()?);
        for kind in kinds {
            seq = seq.next();
            let event = Event { seq, session, kind };
            let _ = table.insert(seq.0, postcard::to_stdvec(&event)?)?;
        }
    }
    write.commit()?;
    Ok(())
}

/// Restores the log boundary immediately before the terminal finish event,
/// leaving the Git refs and delivered issue note as they were at that point.
pub fn lose_terminal_finish(data: &Path, session: SessionId) -> Fallible {
    let database = Database::create(data.join("conductor.redb"))?;
    let write = database.begin_write()?;
    {
        let mut events =
            write.open_table(TableDefinition::<u64, Vec<u8>>::new("events"))?;
        let seq = {
            let (key, value) =
                events.last()?.ok_or("no terminal finish event")?;
            let event: Event = postcard::from_bytes(&value.value())?;
            if event.session != session
                || !matches!(event.kind, EventKind::SessionFinished { .. })
            {
                return Err("the last durable event was not this session's terminal finish".into());
            }
            key.value()
        };
        let _ = events.remove(seq)?;
    }
    write.commit()?;
    Ok(())
}

pub fn rewind_progress_cursor(data: &Path) -> Fallible {
    let database = Database::create(data.join("conductor.redb"))?;
    let write = database.begin_write()?;
    let _ = write
        .open_table(TableDefinition::<&str, Vec<u8>>::new("metadata"))?
        .insert("progress_cursor", postcard::to_stdvec(&HostSeq(0))?)?;
    write.commit()?;
    Ok(())
}
