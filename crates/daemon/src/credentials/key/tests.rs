use std::path::PathBuf;

use super::{Key, LEGACY, NATIVE, Read, SLOT, Vault, open_with, read_file};

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

    fn legacy(&self, key: Key) -> anyhow::Result<()> {
        crate::write_private(
            &self.0.join(LEGACY),
            data_encoding::HEXLOWER.encode(&key).as_bytes(),
        )?;
        Ok(())
    }

    fn open(&self, vault: &mut Mock) -> anyhow::Result<Key> {
        open_with(&self.0, vault, read_file(&self.0, NATIVE)?)
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
fn legacy_migration_reuses_the_key_after_a_write_with_no_reply()
-> anyhow::Result<()> {
    let data = Data::new()?;
    let mut vault = Mock::default();
    let key = [42; 32];
    data.legacy(key)?;
    let sealed = data.0.join("credentials");
    std::fs::write(&sealed, "existing sealed data")?;
    vault.interrupted_write = true;
    assert!(data.open(&mut vault).is_err());
    assert_eq!(read_file(&data.0, LEGACY)?, Some(key));
    assert_eq!(read_file(&data.0, NATIVE)?, None);
    assert!(read_file(&data.0, SLOT)?.is_some());
    vault.interrupted_write = false;
    assert_eq!(data.open(&mut vault)?, key);
    assert_eq!(read_file(&data.0, LEGACY)?, Some(key));
    assert_eq!(data.open(&mut vault)?, key);
    assert_eq!(read_file(&data.0, LEGACY)?, None);
    assert_eq!(vault.writes, 1);
    assert_eq!(std::fs::read_to_string(sealed)?, "existing sealed data");
    Ok(())
}

#[test]
fn active_native_keys_fail_closed_when_missing_locked_or_replaced()
-> anyhow::Result<()> {
    let data = Data::new()?;
    let mut vault = Mock::default();
    let original = data.open(&mut vault)?;
    assert_eq!(data.open(&mut vault)?, original);
    assert_eq!(read_file(&data.0, LEGACY)?, None);
    vault.unavailable = true;
    assert!(data.open(&mut vault).is_err());
    vault.unavailable = false;
    vault.stored = None;
    assert!(data.open(&mut vault).is_err());
    vault.stored = Some([99; 32]);
    assert!(data.open(&mut vault).is_err());
    assert_eq!(vault.writes, 1);
    assert_eq!(read_file(&data.0, LEGACY)?, None);
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
    assert_eq!(data.open(&mut vault)?, first);
    assert_eq!(read_file(&data.0, LEGACY)?, Some(first));
    assert_eq!(read_file(&data.0, NATIVE)?, None);
    std::fs::remove_file(data.0.join(LEGACY))?;
    std::fs::write(data.0.join("credentials"), "keep")?;
    assert!(data.open(&mut vault).is_err());
    vault.unavailable = false;
    assert!(data.open(&mut vault).is_err());
    assert_eq!(vault.writes, 0);
    Ok(())
}

#[test]
fn conflicting_legacy_and_native_keys_are_both_preserved() -> anyhow::Result<()>
{
    let data = Data::new()?;
    let mut vault = Mock::default();
    data.legacy([3; 32])?;
    super::record_marker(&data.0, SLOT, &[1; 32])?;
    vault.stored = Some([4; 32]);
    assert!(data.open(&mut vault).is_err());
    assert_eq!(read_file(&data.0, LEGACY)?, Some([3; 32]));
    assert_eq!(read_file(&data.0, NATIVE)?, None);
    assert_eq!(vault.writes, 0);
    Ok(())
}

#[test]
fn a_file_recovery_requires_the_original_key_after_migration()
-> anyhow::Result<()> {
    let data = Data::new()?;
    let mut vault = Mock::default();
    let original = data.open(&mut vault)?;
    assert_eq!(data.open(&mut vault)?, original);
    std::fs::write(data.0.join(super::FILE_ONLY), "")?;
    assert!(super::open(&data.0).is_err());
    let mut wrong = original;
    wrong[0] ^= 1;
    data.legacy(wrong)?;
    assert!(super::open(&data.0).is_err());
    data.legacy(original)?;
    assert_eq!(super::open(&data.0)?, original);
    assert_eq!(read_file(&data.0, LEGACY)?, Some(original));
    Ok(())
}

#[test]
fn temporary_keyring_loss_after_a_fresh_insert_does_not_create_a_file_key()
-> anyhow::Result<()> {
    let data = Data::new()?;
    let mut vault = Mock {
        interrupted_write: true,
        ..Mock::default()
    };
    assert!(data.open(&mut vault).is_err());
    vault.unavailable = true;
    assert!(data.open(&mut vault).is_err());
    assert_eq!(read_file(&data.0, LEGACY)?, None);
    vault.unavailable = false;
    let key = vault
        .stored
        .ok_or_else(|| anyhow::anyhow!("the mock key was not saved"))?;
    assert_eq!(data.open(&mut vault)?, key);
    Ok(())
}
