use std::fs;
use std::path::{Path, PathBuf};

use host::registry::Change;
use proto::{Catalog, Instance, InstanceId, ModelId, Provider, Roster};

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

#[test]
fn reopening_finishes_adoption_interrupted_after_home() -> anyhow::Result<()> {
    let data = Data::new()?;
    let catalog = Catalog::default();
    let from = InstanceId::own(Provider::Codex);
    let to = InstanceId::named(Provider::Codex, "signed-in");
    let unrelated = InstanceId::named(Provider::Codex, "unrelated");
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
    let (store, _) = Store::open(&data.0, catalog.clone())?;
    store.save(&roster)?;

    data.put("home", &from, "signed-in home")?;
    data.put("home", &to, "old home")?;
    data.put("work", &from, "signed-in work")?;
    let blocked = data.private("work", &to);
    fs::write(&blocked, "blocked")?;

    assert!(store.adopt(&store.signing(&from)?, &to).is_err());
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

#[tokio::test]
async fn terminal_retry_finishes_adoption_interrupted_before_the_roster_save()
-> anyhow::Result<()> {
    let data = Data::new()?;
    let catalog = Catalog::default();
    let from = InstanceId::named(Provider::Codex, "source");
    let to = InstanceId::named(Provider::Codex, "signed-in");
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

    assert!(store.adopt(&store.signing(&from)?, &to).is_err());
    for kind in ["home", "work"] {
        assert!(!data.private(kind, &from).try_exists()?);
        assert_eq!(
            fs::read_to_string(data.private(kind, &to).join("marker"))?,
            "signed-in files"
        );
    }

    fs::remove_dir(blocked)?;
    crate::login(
        Some(data.0.clone()),
        crate::Plugins::default(),
        Provider::Codex,
        Some("source".into()),
    )
    .await?;
    let (_, stored) = Store::open(&data.0, catalog)?;
    let Stored::Opened(roster) = stored else {
        anyhow::bail!("the roster did not reopen");
    };
    assert!(roster.of(&from).is_none());
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
fn competing_adoptions_and_a_stale_credential_update_keep_each_account()
-> anyhow::Result<()> {
    let data = Data::new()?;
    let catalog = Catalog::default();
    let (store, _) = Store::open(&data.0, catalog.clone())?;
    let from = InstanceId::own(Provider::Codex);
    let to = InstanceId::named(Provider::Codex, "codex-account");
    let other_from = InstanceId::own(Provider::Claude);
    let other_to = InstanceId::named(Provider::Claude, "claude-account");
    let credential_id = InstanceId::named(Provider::Claude, "gateway");
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
        let other = scope.spawn(|| other_store.adopt(&other_lease, &other_to));
        store.adopt(&store.signing(&from)?, &to)?;
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
    store.save_change(&roster, &Change::Added(credential_id.clone()))?;

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
    let first = InstanceId::own(Provider::Codex);
    let second = InstanceId::named(Provider::Codex, "second");
    let third = InstanceId::named(Provider::Codex, "third");
    let mut roster = Roster::default();
    roster.0.insert(first.clone(), Instance::default());
    roster.0.insert(second.clone(), Instance::default());
    store.save(&roster)?;
    data.put("home", &first, "first signed-in home")?;
    data.put("home", &second, "second signed-in home")?;

    let (other_store, _) = Store::open(&data.0, catalog)?;
    let first_lease = store.signing(&first)?;
    let second_lease = other_store.signing(&second)?;
    assert!(store.adopt(&first_lease, &second).is_err());
    other_store.adopt(&second_lease, &third)?;
    drop(second_lease);
    store.adopt(&first_lease, &second)?;

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
