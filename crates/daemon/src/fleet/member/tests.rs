use anyhow::Context;
use host::fleet::Sharing;
use proto::{CommandId, CommandResult, Credential, InstanceId, Provider};

use super::{Membership, Mutation, StateRecord, accepted_state};

#[tokio::test]
async fn acceptance_survives_reopen_and_resolves_an_uncertain_write()
-> anyhow::Result<()> {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/tmp")
        .join(format!("acceptance-{}", ulid::Ulid::new()));
    let root_key = iroh::SecretKey::from_bytes(&[31; 32]);
    let member_key = iroh::SecretKey::from_bytes(&[32; 32]);
    let (root, member) = crate::fleet::tests::test_pair(
        &directory.join("root"),
        &root_key,
        &directory.join("member"),
        member_key.public(),
    )
    .await?;
    let mutation = Mutation::Set {
        instance: InstanceId::named(Provider::Claude, "fixture"),
        credential: Credential {
            kind: String::from("fixture"),
            values: [(String::from("value"), String::from("synthetic"))].into(),
        },
        sharing: Sharing::Shared,
    };
    assert_eq!(
        root.accept(CommandId(1), &mutation).await?,
        CommandResult::Accepted
    );
    let version = root.snapshot().await.version;
    let reopened =
        Membership::open(&directory.join("root"), root_key.public())?
            .context("root disappeared")?;
    assert_eq!(
        reopened.accept(CommandId(1), &mutation).await?,
        CommandResult::Accepted
    );
    assert_eq!(reopened.snapshot().await.version, version);
    assert!(matches!(
        member.accept(CommandId(2), &mutation).await?,
        CommandResult::Rejected(_)
    ));
    let removal =
        Mutation::Remove(InstanceId::named(Provider::Claude, "fixture"));
    assert!(matches!(
        reopened.accept(CommandId(1), &removal).await?,
        CommandResult::Rejected(_)
    ));
    let fingerprint =
        super::Sha256::digest(postcard::to_stdvec(&removal)?).into();
    let mut state = reopened.state.write().await;
    let updated = accepted_state(&state, CommandId(2), &removal, fingerprint)?;
    assert!(
        reopened
            .commit_with(&mut state, updated.clone(), |_, _| Err(
                crate::PrivateWriteError {
                    source: std::io::Error::other("before replacement"),
                    replaced: false
                }
            ))
            .is_err()
    );
    assert!(state.pending.is_none());
    assert_eq!(state.current.config.snapshot.version, version);
    assert!(
        reopened
            .commit_with(&mut state, updated, |path, contents| {
                crate::write_private(path, contents).map_err(|source| {
                    crate::PrivateWriteError {
                        source,
                        replaced: false,
                    }
                })?;
                Err(crate::PrivateWriteError {
                    source: std::io::Error::other("after replacement"),
                    replaced: true,
                })
            })
            .is_err()
    );
    assert!(state.pending.is_some());
    assert_eq!(state.current.config.snapshot.version, version);
    drop(state);
    assert_eq!(
        reopened.accept(CommandId(2), &removal).await?,
        CommandResult::Accepted
    );
    assert_eq!(reopened.snapshot().await.version, version + 1);
    let disk = Membership::open(&directory.join("root"), root_key.public())?
        .context("root disappeared")?;
    assert!(disk.credentials().await.0.is_empty());
    assert_eq!(
        disk.accept(CommandId(2), &removal).await?,
        CommandResult::Accepted
    );
    let state = disk.state.read().await;
    let legacy = super::crypto::seal(&disk.store_key, &*state)?;
    let migrated = StateRecord::open(&disk.store_key, &legacy)?;
    assert_eq!(migrated.current, state.current);
    let malformed = super::crypto::seal(&disk.store_key, &(&*state, u64::MAX))?;
    assert!(StateRecord::open(&disk.store_key, &malformed).is_err());
    drop(state);
    std::fs::remove_dir_all(directory)?;
    Ok(())
}

use sha2::Digest;
