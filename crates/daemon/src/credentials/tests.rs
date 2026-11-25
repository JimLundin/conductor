use std::fs;
use std::path::{Path, PathBuf};

use agents::{ModelId, Provider};
use host::registry::{AccountAdoption, Change};
use host::registry::{Catalog, Instance, InstanceId, Roster};

use super::{Store, Stored};

struct Data(PathBuf);

impl Data {
    fn new() -> std::io::Result<Self> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(format!("adoption-{}", ulid::Ulid::new()));
        fs::create_dir_all(&path)?;
        Ok(Self(path))
    }

    fn private(&self, kind: &str, instance: &InstanceId) -> PathBuf {
        self.0.join(kind).join(host::registry::dir(instance))
    }

    fn put(
        &self,
        kind: &str,
        id: &InstanceId,
        value: &str,
    ) -> std::io::Result<()> {
        let dir = self.private(kind, id);
        fs::create_dir_all(&dir)?;
        fs::write(dir.join("marker"), value)
    }
}

impl Drop for Data {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn adoption(from: &InstanceId, to: &InstanceId) -> AccountAdoption {
    AccountAdoption {
        from: from.clone(),
        to: to.clone(),
    }
}

fn adoption_roster(
    from: &InstanceId,
    to: &InstanceId,
    unrelated: &InstanceId,
) -> (Roster, Instance) {
    let mut roster = Roster::default();
    roster.0.insert(
        from.clone(),
        Instance {
            model: Some(ModelId("signed-in model".into())),
            ..Instance::default()
        },
    );
    roster.0.insert(
        to.clone(),
        Instance {
            model: Some(ModelId("previous model".into())),
            ..Instance::default()
        },
    );
    let settings = Instance {
        model: Some(ModelId("unrelated model".into())),
        ..Instance::default()
    };
    roster.0.insert(unrelated.clone(), settings.clone());
    (roster, settings)
}

#[test]
fn reopening_finishes_adoption_interrupted_after_home() -> anyhow::Result<()> {
    let data = Data::new()?;
    let catalog = Catalog::default();
    let from = InstanceId::own(Provider::new("codex"));
    let to = InstanceId::named(Provider::new("codex"), "signed-in");
    let unrelated = InstanceId::named(Provider::new("codex"), "unrelated");
    let (roster, settings) = adoption_roster(&from, &to, &unrelated);
    let (store, _) = Store::open(&data.0, catalog.clone())?;
    store.save(&roster)?;

    data.put("home", &from, "signed-in home")?;
    data.put("home", &to, "old home")?;
    data.put("work", &from, "signed-in work")?;
    let blocked = data.private("work", &to);
    fs::write(&blocked, "blocked")?;

    assert!(
        store
            .adopt(
                &store.signing(&from)?,
                AccountAdoption {
                    from: from.clone(),
                    to: to.clone()
                }
            )
            .is_err()
    );
    assert!(!data.0.join("adoption").try_exists()?);
    for (id, value) in [(&from, "signed-in home"), (&to, "old home")] {
        assert_eq!(
            fs::read_to_string(data.private("home", id).join("marker"))?,
            value
        );
    }
    assert_eq!(fs::read_to_string(&blocked)?, "blocked");

    fs::remove_file(&blocked)?;
    let staged = data.0.join("work/.adoption-source");
    fs::write(&staged, "interrupted")?;
    assert!(
        store
            .adopt(&store.signing(&from)?, adoption(&from, &to))
            .is_err()
    );
    fs::remove_file(staged)?;
    fs::write(&blocked, "new conflict")?;
    let recovery =
        Store::open(&data.0, catalog.clone()).err().ok_or_else(|| {
            anyhow::anyhow!("recovery should expose the conflict")
        })?;
    assert!(
        recovery
            .to_string()
            .contains("resolve the destination conflict")
    );
    assert_eq!(fs::read_to_string(&blocked)?, "new conflict");
    assert_eq!(
        fs::read_to_string(data.0.join("home/.adoption-backup/marker"))?,
        "old home"
    );
    assert_eq!(
        fs::read_to_string(data.private("work", &from).join("marker"))?,
        "signed-in work"
    );
    assert!(!data.private("home", &from).try_exists()?);
    assert_eq!(
        fs::read_to_string(data.private("home", &to).join("marker"))?,
        "signed-in home"
    );

    fs::remove_file(blocked)?;
    data.put("work", &to, "old work")?;
    let (_, stored) = Store::open(&data.0, catalog)?;
    let Stored::Opened(roster) = stored else {
        anyhow::bail!("the roster did not reopen");
    };
    for (kind, value) in
        [("home", "signed-in home"), ("work", "signed-in work")]
    {
        assert_eq!(
            fs::read_to_string(data.private(kind, &to).join("marker"))?,
            value
        );
    }
    assert!(roster.of(&from).is_some());
    assert_eq!(
        roster.of(&to).and_then(|entry| entry.model.as_ref()),
        Some(&ModelId("signed-in model".into()))
    );
    assert_eq!(roster.of(&unrelated), Some(&settings));
    Ok(())
}

#[test]
fn retry_installs_the_staged_directory_after_the_destination_is_recreated()
-> anyhow::Result<()> {
    let data = Data::new()?;
    let catalog = Catalog::default();
    let from = InstanceId::named(Provider::new("codex"), "source");
    let to = InstanceId::named(Provider::new("codex"), "signed-in");
    let mut roster = Roster::default();
    roster.0.insert(from.clone(), Instance::default());
    roster.0.insert(to.clone(), Instance::default());
    let (store, _) = Store::open(&data.0, catalog)?;
    store.save(&roster)?;
    data.put("work", &from, "signed-in files")?;

    let destination = data.private("work", &to);
    data.put("work", &to, "previous destination")?;
    let backup = data.0.join("work/.adoption-backup");
    fs::write(&backup, "blocked")?;
    assert!(
        store
            .adopt(
                &store.signing(&from)?,
                AccountAdoption {
                    from: from.clone(),
                    to: to.clone()
                }
            )
            .is_err()
    );
    fs::remove_file(&backup)?;
    fs::rename(&destination, &backup)?;
    data.put("work", &to, "recreated destination")?;

    let blocked = data.0.join("credentials.new");
    fs::create_dir(&blocked)?;
    assert!(store.resume_adoption(|_| true).is_err());
    assert_eq!(
        fs::read_to_string(backup.join("marker"))?,
        "previous destination"
    );
    assert_eq!(
        fs::read_to_string(destination.join("marker"))?,
        "signed-in files"
    );
    fs::remove_dir(blocked)?;
    assert!(store.resume_adoption(|_| true)?.is_some());
    assert!(!backup.try_exists()?);
    Ok(())
}

#[tokio::test]
async fn terminal_retry_finishes_adoption_interrupted_before_the_roster_save()
-> anyhow::Result<()> {
    let data = Data::new()?;
    let catalog = Catalog::default();
    let from = InstanceId::named(Provider::new("codex"), "source");
    let to = InstanceId::named(Provider::new("codex"), "signed-in");
    let mut roster = Roster::default();
    roster.0.insert(from.clone(), Instance::default());
    let (store, _) = Store::open(&data.0, catalog.clone())?;
    store.save(&roster)?;

    for kind in ["home", "work"] {
        data.put(kind, &from, "signed-in files")?;
        data.put(kind, &to, "previous files")?;
    }
    let blocked = data.0.join("credentials.new");
    fs::create_dir(&blocked)?;

    assert!(
        store
            .adopt(
                &store.signing(&from)?,
                AccountAdoption {
                    from: from.clone(),
                    to: to.clone()
                }
            )
            .is_err()
    );
    for kind in ["home", "work"] {
        assert!(!data.private(kind, &from).try_exists()?);
        assert_eq!(
            fs::read_to_string(data.private(kind, &to).join("marker"))?,
            "signed-in files"
        );
    }

    fs::remove_dir(blocked)?;
    for kind in ["home", "work"] {
        data.put(kind, &from, "recreated source")?;
    }
    crate::login(
        Some(data.0.clone()),
        crate::Plugins::default(),
        Provider::new("codex"),
        Some("source".into()),
    )
    .await?;
    let (_, stored) = Store::open(&data.0, catalog)?;
    let Stored::Opened(roster) = stored else {
        anyhow::bail!("the roster did not reopen");
    };
    assert!(roster.of(&from).is_none());
    for kind in ["home", "work"] {
        assert_eq!(
            fs::read_to_string(data.private(kind, &from).join("marker"))?,
            "recreated source"
        );
    }
    assert!(roster.of(&to).is_some());
    for kind in ["home", "work"] {
        assert_eq!(
            fs::read_to_string(data.private(kind, &to).join("marker"))?,
            "signed-in files"
        );
    }
    Ok(())
}

#[test]
fn committed_adoption_returns_its_roster_while_cleanup_can_retry()
-> anyhow::Result<()> {
    let data = Data::new()?;
    let catalog = Catalog::default();
    let from = InstanceId::named(Provider::new("codex"), "source");
    let to = InstanceId::named(Provider::new("codex"), "signed-in");
    let mut roster = Roster::default();
    roster.0.insert(from.clone(), Instance::default());
    let (store, _) = Store::open(&data.0, catalog.clone())?;
    store.save(&roster)?;
    data.put("home", &from, "signed-in files")?;

    let blocked = data.0.join("credentials.new");
    fs::create_dir(&blocked)?;
    assert!(
        store
            .adopt(
                &store.signing(&from)?,
                AccountAdoption {
                    from: from.clone(),
                    to: to.clone()
                }
            )
            .is_err()
    );
    fs::remove_dir(blocked)?;

    let backup = data.0.join("home/.adoption-backup");
    fs::write(&backup, "cleanup blocked")?;
    let committed = store
        .resume_adoption(|_| true)?
        .ok_or_else(|| anyhow::anyhow!("pending adoption"))?;
    assert!(committed.roster.of(&from).is_none());
    assert!(committed.roster.of(&to).is_some());

    fs::remove_file(backup)?;
    let retried = store
        .resume_adoption(|_| true)?
        .ok_or_else(|| anyhow::anyhow!("cleanup pending"))?;
    assert_eq!(retried.roster, committed.roster);
    assert!(store.resume_adoption(|_| true)?.is_none());
    let (_, stored) = Store::open(&data.0, catalog)?;
    assert_eq!(stored, Stored::Opened(committed.roster));
    assert_eq!(
        fs::read_to_string(data.private("home", &to).join("marker"))?,
        "signed-in files"
    );
    Ok(())
}

#[test]
fn a_signing_lease_uses_the_current_roster() -> anyhow::Result<()> {
    let data = Data::new()?;
    let catalog = Catalog::default();
    let instance = InstanceId::named(Provider::new("codex"), "source");
    let mut roster = Roster::default();
    roster.0.insert(instance.clone(), Instance::default());
    let (writer, _) = Store::open(&data.0, catalog.clone())?;
    writer.save(&roster)?;
    let (store, opened) = Store::open(&data.0, catalog)?;
    let updated = Instance {
        model: Some(ModelId("current model".into())),
        ..Instance::default()
    };
    roster.0.insert(instance.clone(), updated.clone());
    writer.save(&roster)?;

    assert_eq!(
        opened.roster(&Catalog::default()).of(&instance),
        Some(&Instance::default())
    );
    assert_eq!(store.signing(&instance)?.settings, updated);
    Ok(())
}

#[test]
fn competing_adoptions_and_a_stale_credential_update_keep_each_account()
-> anyhow::Result<()> {
    let data = Data::new()?;
    let catalog = Catalog::default();
    let (store, _) = Store::open(&data.0, catalog.clone())?;
    let from = InstanceId::own(Provider::new("codex"));
    let to = InstanceId::named(Provider::new("codex"), "codex-account");
    let other_from = InstanceId::own(Provider::new("claude"));
    let other_to = InstanceId::named(Provider::new("claude"), "claude-account");
    let credential_id = InstanceId::named(Provider::new("claude"), "gateway");
    let source_settings = Instance {
        model: Some(ModelId("first model".into())),
        ..Instance::default()
    };
    let other_settings = Instance {
        model: Some(ModelId("second model".into())),
        ..Instance::default()
    };
    let mut roster = Roster::default();
    roster.0.insert(from.clone(), source_settings.clone());
    roster.0.insert(other_from.clone(), other_settings.clone());
    store.save(&roster)?;

    data.put("home", &from, "codex files")?;
    data.put("work", &to, "existing work")?;
    data.put("home", &other_from, "claude files")?;

    let (other_store, _) = Store::open(&data.0, catalog.clone())?;
    let other_lease = other_store.signing(&other_from)?;
    std::thread::scope(|scope| {
        let other = scope.spawn(|| {
            other_store.adopt(&other_lease, adoption(&other_from, &other_to))
        });
        store.adopt(&store.signing(&from)?, adoption(&from, &to))?;
        other
            .join()
            .map_err(|_| anyhow::anyhow!("the adoption thread panicked"))??;
        Ok::<_, anyhow::Error>(())
    })?;

    let added = Instance {
        model: Some(ModelId("gateway model".into())),
        ..Instance::default()
    };
    roster.0.insert(credential_id.clone(), added.clone());
    store.save_change(
        &roster,
        &Change::Added(credential_id.clone()),
        None,
        None,
    )?;

    let (_, stored) = Store::open(&data.0, catalog)?;
    let Stored::Opened(roster) = stored else {
        anyhow::bail!("the roster did not reopen");
    };
    assert_eq!(roster.of(&from), Some(&source_settings));
    assert_eq!(roster.of(&to), Some(&source_settings));
    assert_eq!(roster.of(&other_from), Some(&other_settings));
    assert_eq!(roster.of(&other_to), Some(&other_settings));
    assert_eq!(roster.of(&credential_id), Some(&added));
    assert_eq!(
        fs::read_to_string(data.private("work", &to).join("marker"))?,
        "existing work"
    );
    Ok(())
}

#[test]
fn adoption_keeps_the_source_of_another_sign_in() -> anyhow::Result<()> {
    let data = Data::new()?;
    let catalog = Catalog::default();
    let (store, _) = Store::open(&data.0, catalog.clone())?;
    let first = InstanceId::own(Provider::new("codex"));
    let second = InstanceId::named(Provider::new("codex"), "second");
    let third = InstanceId::named(Provider::new("codex"), "third");
    let mut roster = Roster::default();
    roster.0.insert(first.clone(), Instance::default());
    roster.0.insert(second.clone(), Instance::default());
    store.save(&roster)?;
    data.put("home", &first, "first signed-in home")?;
    data.put("home", &second, "second signed-in home")?;

    let (other_store, _) = Store::open(&data.0, catalog)?;
    let first_lease = store.signing(&first)?;
    let second_lease = other_store.signing(&second)?;
    assert!(
        store
            .adopt(
                &first_lease,
                AccountAdoption {
                    from: first.clone(),
                    to: second.clone()
                }
            )
            .is_err()
    );
    other_store.adopt(&second_lease, adoption(&second, &third))?;
    drop(second_lease);
    store.adopt(&first_lease, adoption(&first, &second))?;

    for (id, contents) in [
        (&second, "first signed-in home"),
        (&third, "second signed-in home"),
    ] {
        assert_eq!(
            fs::read_to_string(data.private("home", id).join("marker"))?,
            contents
        );
    }
    Ok(())
}

#[test]
fn signing_distinguishes_empty_and_unreadable_rosters() -> anyhow::Result<()> {
    let data = Data::new()?;
    let (store, _) = Store::open(&data.0, Catalog::default())?;
    store.save(&Roster::default())?;
    let id = InstanceId::own(Provider::new("codex"));
    let empty = store
        .signing(&id)
        .err()
        .ok_or_else(|| anyhow::anyhow!("empty roster"))?;
    assert!(empty.to_string().contains("this host has no instance"));

    data.put("home", &id, "keep this home")?;
    let credentials = data.0.join("credentials");
    fs::write(&credentials, "synthetic damaged roster")?;
    let unreadable = store
        .signing(&id)
        .err()
        .ok_or_else(|| anyhow::anyhow!("unreadable roster"))?;
    assert!(
        unreadable
            .to_string()
            .contains("does not open with the store key")
    );
    assert_eq!(fs::read_to_string(credentials)?, "synthetic damaged roster");
    assert_eq!(
        fs::read_to_string(data.private("home", &id).join("marker"))?,
        "keep this home"
    );
    Ok(())
}

#[test]
fn removal_result_survives_reopening_before_the_command_receipt()
-> anyhow::Result<()> {
    let data = Data::new()?;
    let catalog = Catalog::default();
    let id = link::CommandId(uuid::Uuid::from_u128(17));
    let instance = InstanceId::named(Provider::new("codex"), "removed");
    let (store, _) = Store::open(&data.0, catalog.clone())?;
    let mut roster = Roster::default();
    roster.0.insert(instance.clone(), Instance::default());
    store.save(&roster)?;
    store.save_change(
        &Roster::default(),
        &Change::Removed(instance.clone()),
        None,
        Some(id),
    )?;
    drop(store);

    let (store, stored) = Store::open(&data.0, catalog)?;
    assert!(stored.roster(&Catalog::default()).of(&instance).is_none());
    assert!(store.removal(id));
    assert!(!store.removal(link::CommandId(uuid::Uuid::from_u128(18))));
    Ok(())
}

#[test]
fn a_store_sealed_in_the_previous_format_is_unreadable_and_moves_aside()
-> anyhow::Result<()> {
    use chacha20poly1305::XNonce;
    use chacha20poly1305::aead::{Aead, Payload};

    let data = Data::new()?;
    let (store, _) = Store::open(&data.0, Catalog::default())?;
    let mut credentials = super::Credentials::default();
    credentials.roster.0.insert(
        InstanceId::named(Provider::new("codex"), "kept"),
        Instance::default(),
    );
    let nonce = [5; super::NONCE_LEN];
    let sealed = store
        .cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: &postcard::to_stdvec(&credentials)?,
                aad: super::PURPOSE,
            },
        )
        .map_err(|_| anyhow::anyhow!("sealing"))?;
    let previous = std::iter::once(7)
        .chain(nonce)
        .chain(sealed)
        .collect::<Vec<u8>>();
    fs::write(data.0.join("credentials"), &previous)?;
    drop(store);

    let (store, stored) = Store::open(&data.0, Catalog::default())?;
    assert!(matches!(stored, Stored::Unreadable(_)));
    let aside = store.move_aside()?;
    assert_eq!(aside, data.0.join("credentials.unreadable"));
    assert_eq!(fs::read(aside)?, previous);
    drop(store);
    let (_, stored) = Store::open(&data.0, Catalog::default())?;
    assert!(matches!(stored, Stored::Fresh));
    Ok(())
}

#[test]
fn an_unversioned_adoption_journal_is_refused_and_kept() -> anyhow::Result<()> {
    let data = Data::new()?;
    let from = InstanceId::named(Provider::new("codex"), "source");
    let to = InstanceId::named(Provider::new("codex"), "signed-in");
    data.put("home", &from, "signed-in files")?;
    let journal = data.0.join("adoption");
    let home_from_source = 1u8;
    let work_absent = 0;
    let unversioned = postcard::to_stdvec(&(
        adoption(&from, &to),
        [home_from_source, work_absent],
    ))?;
    fs::write(&journal, &unversioned)?;

    let refused = Store::open(&data.0, Catalog::default())
        .err()
        .ok_or_else(|| anyhow::anyhow!("an unversioned journal opened"))?;
    assert!(format!("{refused:#}").contains("is in an older format"));
    assert_eq!(fs::read(&journal)?, unversioned);
    assert_eq!(
        fs::read_to_string(data.private("home", &from).join("marker"))?,
        "signed-in files"
    );
    Ok(())
}
