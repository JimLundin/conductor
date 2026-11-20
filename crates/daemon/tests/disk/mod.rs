use std::path::Path;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use proto::{Epoch, Event, EventKind, HostSeq, Roster, SessionId};
use redb::{Database, ReadableTableMetadata, TableDefinition};

use crate::Fallible;

const HEADER: u8 = 5;
const PURPOSE: &[u8] = b"conductor credentials 1";

pub fn seed_roster(data: &Path, roster: &Roster) -> Fallible {
    let mut key = [0; 32];
    let mut nonce = [0; 24];
    getrandom::fill(&mut key)?;
    getrandom::fill(&mut nonce)?;
    let plain = postcard::to_stdvec(roster)?;
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
    let (nonce, encrypted) = file
        .strip_prefix(&[HEADER])
        .and_then(|body| body.split_at_checked(24))
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
    Ok(postcard::from_bytes(&plain)?)
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
