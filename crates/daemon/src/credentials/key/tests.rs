use std::path::PathBuf;

use super::{FILE, FILE_ONLY, Key, Read, SLOT, Vault, open_with, read_file};

struct Data(PathBuf);

impl Data {
    fn new() -> anyhow::Result<Self> {
        let dir = std::env::current_exe()?
            .ancestors()
            .nth(3)
            .ok_or_else(|| {
                anyhow::anyhow!("test binary is not in target/debug/deps")
            })?
            .join("tmp")
            .join(format!("keystore-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }

    fn open(&self, vault: &mut Mock) -> anyhow::Result<Key> {
        open_with(&self.0, vault)
    }
}

impl Drop for Data {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct Mock {
    stored: Option<Key>,
    unavailable: bool,
    interrupted_write: bool,
    writes: usize,
}

impl Vault for Mock {
    fn read(&self, account: &str) -> anyhow::Result<Read> {
        assert_eq!(account.len(), 64);
        Ok(if self.unavailable {
            Read::Unavailable
        } else {
            self.stored.map_or(Read::Missing, Read::Found)
        })
    }

    fn insert(&mut self, _: &str, key: &Key) -> anyhow::Result<()> {
        self.stored = Some(*key);
        self.writes += 1;
        anyhow::ensure!(!self.interrupted_write, "interrupted save");
        Ok(())
    }
}

#[test]
fn a_new_store_keeps_its_key_in_the_keystore_and_no_file() -> anyhow::Result<()>
{
    let data = Data::new()?;
    let mut vault = Mock::default();
    let key = data.open(&mut vault)?;
    assert_eq!(data.open(&mut vault)?, key);
    assert_eq!(vault.writes, 1);
    assert!(read_file(&data.0, SLOT)?.is_some());
    assert_eq!(read_file(&data.0, FILE)?, None);
    Ok(())
}

#[test]
fn a_keystore_store_fails_closed_when_its_entry_is_missing_or_locked()
-> anyhow::Result<()> {
    let data = Data::new()?;
    let mut vault = Mock::default();
    data.open(&mut vault)?;
    vault.unavailable = true;
    assert!(data.open(&mut vault).is_err());
    vault.unavailable = false;
    vault.stored = None;
    assert!(data.open(&mut vault).is_err());
    assert_eq!(read_file(&data.0, FILE)?, None);
    assert_eq!(vault.writes, 1);
    Ok(())
}

#[test]
fn an_interrupted_save_records_no_slot_and_the_next_start_begins_again()
-> anyhow::Result<()> {
    let data = Data::new()?;
    let mut vault = Mock {
        interrupted_write: true,
        ..Mock::default()
    };
    assert!(data.open(&mut vault).is_err());
    assert_eq!(read_file(&data.0, SLOT)?, None);
    vault.interrupted_write = false;
    vault.stored = None;
    let key = data.open(&mut vault)?;
    assert_eq!(vault.stored, Some(key));
    assert!(read_file(&data.0, SLOT)?.is_some());
    Ok(())
}

#[test]
fn headless_hosts_keep_the_private_file_and_do_not_rekey_a_sealed_store()
-> anyhow::Result<()> {
    let data = Data::new()?;
    let mut vault = Mock {
        unavailable: true,
        ..Mock::default()
    };
    let first = data.open(&mut vault)?;
    vault.unavailable = false;
    assert_eq!(data.open(&mut vault)?, first);
    assert_eq!(read_file(&data.0, FILE)?, Some(first));
    assert_eq!(read_file(&data.0, SLOT)?, None);
    std::fs::remove_file(data.0.join(FILE))?;
    std::fs::write(data.0.join("credentials"), "keep")?;
    assert!(data.open(&mut vault).is_err());
    assert_eq!(vault.writes, 0);
    Ok(())
}

#[test]
fn the_file_marker_chooses_the_private_file_for_a_new_store()
-> anyhow::Result<()> {
    let data = Data::new()?;
    let mut vault = Mock::default();
    std::fs::write(data.0.join(FILE_ONLY), "")?;
    let key = data.open(&mut vault)?;
    assert_eq!(read_file(&data.0, FILE)?, Some(key));
    assert_eq!(vault.writes, 0);
    Ok(())
}
