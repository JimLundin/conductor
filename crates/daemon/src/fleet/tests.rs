use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, ensure};
use host::fleet::{
    Certificate, DeviceHello, HostEntry, Reply, SignedConfiguration,
};
use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh::{Endpoint, EndpointId, RelayMode, SecretKey};

use super::Membership;
use super::member::clock;
use super::peer::{Protocol, admit_existing, join, sync_with};
use fleet_store::crypto;

async fn endpoint(seed: u8) -> anyhow::Result<Endpoint> {
    Ok(Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .secret_key(SecretKey::from_bytes(&[seed; 32]))
        .bind()
        .await?)
}

fn dir() -> anyhow::Result<PathBuf> {
    let binary = std::env::current_exe()?;
    let target = binary
        .ancestors()
        .nth(3)
        .context("test binary is not in target/debug/deps")?;
    let nonce = getrandom::u64()?;
    let path = target.join("tmp").join(format!("fleet-test-{nonce:016x}"));
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

fn entry(endpoint: &Endpoint, name: &str) -> HostEntry {
    host_entry(endpoint.id(), name)
}

fn host_entry(id: EndpointId, name: &str) -> HostEntry {
    HostEntry {
        node: *id.as_bytes(),
        name: name.into(),
        relay: "https://relay.example.invalid/".into(),
        added_at: 0,
    }
}

async fn sync(
    endpoint: &Endpoint,
    member: &Membership,
    remote: &Endpoint,
) -> anyhow::Result<()> {
    tokio::time::timeout(
        Duration::from_secs(10),
        sync_with(endpoint, member, remote.addr()),
    )
    .await?
}

#[tokio::test]
async fn loopback_join_replays_for_one_host_and_propagates_revocations()
-> anyhow::Result<()> {
    let path = dir()?;
    let root_endpoint = endpoint(1).await?;
    let member_endpoint = endpoint(2).await?;
    let outsider = endpoint(3).await?;
    let backup = path.join("root-backup");
    let root_dir = path.join("root");
    let member_dir = path.join("member");
    let phrase = b"synthetic test passphrase";
    let root = Membership::initialize(
        &root_dir,
        entry(&root_endpoint, "root"),
        &SecretKey::from_bytes(&[1; 32]),
        &backup,
        phrase,
    )?;
    let root_public = root.snapshot().await.fleet;
    let restored = check_backup(&backup, phrase, &root_public)?;

    let router = Router::builder(root_endpoint.clone())
        .accept(host::fleet::ALPN, Protocol::for_member(root.clone()))
        .spawn();
    let now = clock()?;
    let (member, certificate, config) = enroll_member(
        &root,
        &root_endpoint,
        &member_endpoint,
        &outsider,
        &member_dir,
        now,
    )
    .await?;

    let (hello, owner) = check_owner_proof(
        &root,
        &member,
        &root_public,
        root_endpoint.id(),
        member_endpoint.id(),
        outsider.id(),
    )
    .await?;
    let (witness, witness_router) =
        start_witness(&root, &root_endpoint, &outsider, &path, now).await?;
    sync(&member_endpoint, &member, &root_endpoint).await?;
    verify_replication(
        &root,
        &member,
        &witness,
        &root_endpoint,
        &member_endpoint,
        &outsider,
        owner,
    )
    .await?;
    assert!(member.admit_device(outsider.id(), &hello).await.is_err());
    assert!(!member.adopt(config).await?);
    assert_eq!(member.snapshot().await.version, 6);

    root.revoke(*member_endpoint.id().as_bytes()).await?;
    tokio::time::timeout(
        Duration::from_secs(10),
        sync_with(&member_endpoint, &member, root_endpoint.addr()),
    )
    .await??;
    assert!(!member.is_active().await);
    assert!(
        root.admit_host(member_endpoint.id(), &certificate)
            .await
            .is_err()
    );
    check_recovery(
        &root,
        &path,
        phrase,
        &restored,
        &hello.certificate,
        root_endpoint.id(),
    )
    .await?;
    let reopened = Membership::open(&member_dir, member_endpoint.id())?
        .context("missing local membership")?;
    ensure!(
        !reopened.is_active().await,
        "host reactivated after reopening the store"
    );
    router.shutdown().await?;
    member_endpoint.close().await;
    witness_router.shutdown().await?;
    std::fs::remove_dir_all(path)?;
    Ok(())
}

fn check_backup(
    backup: &Path,
    phrase: &[u8],
    root_public: &host::fleet::NodeId,
) -> anyhow::Result<super::member::Recovery> {
    let restored: super::member::Recovery = crypto::recover(backup, phrase)?;
    assert_eq!(
        SecretKey::from_bytes(&restored.root_key)
            .public()
            .as_bytes(),
        root_public
    );
    assert!(
        crypto::recover::<super::member::Recovery>(
            backup,
            b"incorrect fixture passphrase"
        )
        .is_err()
    );

    Ok(restored)
}

async fn enroll_member(
    root: &Membership,
    root_endpoint: &Endpoint,
    member_endpoint: &Endpoint,
    outsider: &Endpoint,
    member_dir: &Path,
    now: u64,
) -> anyhow::Result<(Membership, Certificate, SignedConfiguration)> {
    let invite = root.invite(now).await?;
    let member_entry = entry(member_endpoint, "member");
    let Reply::Joined {
        certificate,
        config,
    } = join(
        member_endpoint,
        root_endpoint.addr(),
        &invite,
        member_entry.clone(),
    )
    .await?
    else {
        anyhow::bail!("root refused a current invitation");
    };
    let retried =
        join(member_endpoint, root_endpoint.addr(), &invite, member_entry)
            .await?;
    assert_eq!(
        retried,
        Reply::Joined {
            certificate: certificate.clone(),
            config: config.clone()
        }
    );
    let denied = join(
        outsider,
        root_endpoint.addr(),
        &invite,
        entry(outsider, "outsider"),
    )
    .await?;
    assert_eq!(denied, Reply::Refused);
    assert!(root.admit_host(outsider.id(), &certificate).await.is_err());

    let member = Membership::install_joined(
        member_dir,
        member_endpoint.id(),
        &invite,
        certificate.clone(),
        config.clone(),
    )?;
    assert!(member.invite(now).await.is_err());
    assert!(
        root.join(
            member_endpoint.id(),
            &invite.secret,
            entry(member_endpoint, "member"),
            now + 3600
        )
        .await
        .is_err()
    );

    Ok((member, certificate, config))
}

async fn check_owner_proof(
    root: &Membership,
    member: &Membership,
    root_public: &host::fleet::NodeId,
    root_id: EndpointId,
    member_id: EndpointId,
    outsider: EndpointId,
) -> anyhow::Result<(DeviceHello, host::fleet::NodeId)> {
    let owner = SecretKey::from_bytes(&[4; 32]);
    let owner_cert = root
        .sign_owner(*owner.public().as_bytes(), "phone".into())
        .await?;
    let proof = host::fleet::device_proof(
        root_public,
        member_id.as_bytes(),
        outsider.as_bytes(),
    );
    let hello = DeviceHello {
        certificate: owner_cert,
        proof: owner.sign(&proof).to_bytes().to_vec(),
    };
    let admitted = member.admit_device(outsider, &hello).await?;
    assert_eq!(admitted.name, "phone");
    assert!(member.admit_device(root_id, &hello).await.is_err());
    Ok((hello, admitted.device))
}

async fn start_witness(
    root: &Membership,
    root_endpoint: &Endpoint,
    outsider: &Endpoint,
    path: &Path,
    now: u64,
) -> anyhow::Result<(Membership, Router)> {
    let witness_invite = root.invite(now).await?;
    let Reply::Joined {
        certificate: witness_cert,
        config: witness_config,
    } = join(
        outsider,
        root_endpoint.addr(),
        &witness_invite,
        entry(outsider, "witness"),
    )
    .await?
    else {
        anyhow::bail!("root refused a second invitation");
    };
    let witness = Membership::install_joined(
        &path.join("witness"),
        outsider.id(),
        &witness_invite,
        witness_cert,
        witness_config,
    )?;
    let witness_router = Router::builder(outsider.clone())
        .accept(host::fleet::ALPN, Protocol::for_member(witness.clone()))
        .spawn();
    Ok((witness, witness_router))
}

async fn verify_replication(
    root: &Membership,
    member: &Membership,
    witness: &Membership,
    root_endpoint: &Endpoint,
    member_endpoint: &Endpoint,
    outsider: &Endpoint,
    owner: host::fleet::NodeId,
) -> anyhow::Result<()> {
    let instance = host::registry::InstanceId::named(
        agents::Provider::new("claude"),
        "fixture",
    );
    root.change_credentials(|roster| {
        let key = agents::Credential {
            kind: String::from("api_key"),
            values: [(String::from("key"), String::from("synthetic-value"))]
                .into(),
        };
        roster.0.insert(
            instance.clone(),
            host::registry::Instance {
                credential: Some(key),
                ..host::registry::Instance::default()
            },
        );
    })
    .await?;
    publish(root, Some("fixture-replicated")).await?;
    root.revoke(owner).await?;
    sync(outsider, witness, root_endpoint).await?;

    let mut forged = root
        .config_after(0)
        .await
        .context("missing signed configuration")?;
    forged.config.snapshot.version += 1;
    assert!(member.adopt(forged).await.is_err());

    sync(member_endpoint, member, outsider).await?;
    assert!(member.credentials().await.of(&instance).is_some());
    let latest = member
        .config_after(0)
        .await
        .context("missing configuration")?;
    assert_eq!(
        latest.config.project_credentials.get(SHARED),
        Some(&host::fleet::ProjectCredential::new(
            "fixture-replicated".to_owned()
        ))
    );
    let public = postcard::to_stdvec(&member.public_snapshot().await)?;
    assert!(
        !public
            .windows(b"synthetic-value".len())
            .any(|bytes| bytes == b"synthetic-value")
    );
    Ok(())
}

async fn check_recovery(
    root: &Membership,
    path: &Path,
    phrase: &[u8],
    restored: &super::member::Recovery,
    owner: &Certificate,
    root_id: EndpointId,
) -> anyhow::Result<()> {
    let export = path.join("latest-configuration");
    root.export_config(&export, phrase).await?;
    let recovered_dir = path.join("recovered");
    std::fs::create_dir_all(&recovered_dir)?;
    let recovered_root = Membership::restore_root(
        &recovered_dir,
        restored,
        Some(crypto::recover(&export, phrase)?),
    )?;
    assert_eq!(recovered_root.snapshot().await.version, 7);
    check_shared(&recovered_dir, "fixture-replicated")?;
    assert!(recovered_root.admit_owner(owner).await.is_err());
    assert_eq!(crate::load_or_create_key(&recovered_dir)?.public(), root_id);
    Ok(())
}

#[tokio::test]
async fn standalone_admission_and_browser_approval_use_the_same_root()
-> anyhow::Result<()> {
    let path = dir()?;
    let root_endpoint = endpoint(11).await?;
    let target = endpoint(12).await?;
    let root = Membership::initialize(
        &path.join("root"),
        entry(&root_endpoint, "root"),
        &SecretKey::from_bytes(&[11; 32]),
        &path.join("backup"),
        b"fixture backup passphrase",
    )?;
    let target_path = path.join("standalone");
    let legacy = host::fleet::EnrollmentToken::from_bytes([7; 16]);
    let (slot, _) = tokio::sync::watch::channel(None);
    let target_router = Router::builder(target.clone())
        .accept(
            host::fleet::ALPN,
            Protocol::new(slot.clone(), target_path, target.id(), legacy),
        )
        .spawn();
    let target_host = entry(&target, "standalone");
    let wrong = host::fleet::EnrollmentToken::from_bytes([8; 16]);
    assert!(
        admit_existing(
            &root_endpoint,
            &root,
            target.addr(),
            wrong,
            target_host.clone()
        )
        .await
        .is_err()
    );
    assert_eq!(root.snapshot().await.version, 1);
    assert!(slot.borrow().is_none());

    admit_existing(
        &root_endpoint,
        &root,
        target.addr(),
        legacy,
        target_host.clone(),
    )
    .await?;
    admit_existing(&root_endpoint, &root, target.addr(), legacy, target_host)
        .await?;
    assert_eq!(root.snapshot().await.version, 2);
    let member = slot
        .borrow()
        .clone()
        .context("standalone host did not switch its membership")?;

    let request = host::fleet::PairRequest {
        device: *SecretKey::from_bytes(&[13; 32]).public().as_bytes(),
        name: String::from("new browser"),
    };
    let action = host::fleet::FleetCommand::Approve(request);
    let reply =
        super::command(Some(&root), &root_endpoint, action.clone()).await;
    let host::fleet::FleetResult::Pairing(pairing) = reply else {
        anyhow::bail!("root refused browser approval");
    };
    member.admit_owner(&pairing.certificate).await?;
    assert!(matches!(
        super::command(Some(&member), &target, action).await,
        host::fleet::FleetResult::Unavailable
    ));

    target_router.shutdown().await?;
    std::fs::remove_dir_all(path)?;
    Ok(())
}

const SHARED: &str = "fleet/example";

async fn publish(root: &Membership, token: Option<&str>) -> anyhow::Result<()> {
    let entry = token
        .map(|value| host::fleet::ProjectCredential::new(value.to_owned()));
    root.change_project_credential(SHARED.to_owned(), entry)
        .await
}

fn check_shared(data: &Path, expected: &str) -> anyhow::Result<()> {
    use project_runtime::transport::SharedCredential;
    use secrecy::ExposeSecret as _;
    let resolved = fleet_store::project_credential(data, SHARED)?;
    ensure!(!format!("{resolved:?}").contains(expected));
    let SharedCredential::Stored(entry) = resolved else {
        anyhow::bail!("no shared credential");
    };
    ensure!(
        entry.expose_secret() == expected,
        "unexpected shared credential"
    );
    Ok(())
}

async fn latest(root: &Membership, member: &Membership) -> anyhow::Result<()> {
    let config = root
        .config_after(member.snapshot().await.version)
        .await
        .context("missing update")?;
    assert!(member.adopt(config).await?);
    Ok(())
}

#[tokio::test]
async fn hosts_share_rotate_withdraw_and_revoke_project_credentials()
-> anyhow::Result<()> {
    use project_runtime::transport::SharedCredential;

    let path = dir()?;
    let data = path.join("root");
    let member_dir = path.join("member");
    let root_id = crate::load_or_create_key(&data)?;
    let member_id = crate::load_or_create_key(&member_dir)?.public();
    assert!(matches!(
        fleet_store::project_credential(&data, SHARED)?,
        SharedCredential::Standalone
    ));
    Membership::initialize(
        &data,
        host_entry(root_id.public(), "root"),
        &root_id,
        &path.join("backup"),
        b"fixture passphrase",
    )?;
    assert!(matches!(
        fleet_store::project_credential(&data, SHARED)?,
        SharedCredential::Missing
    ));
    let original = "fixture-github-original";
    super::change_project_credential(
        &data,
        SHARED.to_owned(),
        Some(original.into()),
    )
    .await?;
    let root = super::local(&data)?;
    let version = root.snapshot().await.version;
    publish(&root, Some(original)).await?;
    assert_eq!(root.snapshot().await.version, version);
    check_shared(&data, original)?;
    let invite = root.invite(1).await?;
    let (certificate, config) = root
        .join(
            member_id,
            &invite.secret,
            host_entry(member_id, "member"),
            2,
        )
        .await?;
    ensure!(!format!("{config:?}").contains(original));
    let mut tampered = config.clone();
    tampered.config.project_credentials.clear();
    assert!(
        crypto::verify_config(
            iroh::PublicKey::from_bytes(&invite.fleet)?,
            &tampered
        )
        .is_err()
    );
    let public = std::str::from_utf8(&config.public.payload)?;
    assert!(!public.contains(original));
    assert!(!public.contains(SHARED));
    let member = Membership::install_joined(
        &member_dir,
        member_id,
        &invite,
        certificate,
        config,
    )?;
    check_shared(&member_dir, original)?;
    assert!(
        super::change_project_credential(&member_dir, SHARED.to_owned(), None)
            .await
            .is_err()
    );
    let rotated = "fixture-github-rotated";
    publish(&root, Some(rotated)).await?;
    check_shared(&member_dir, original)?;
    latest(&root, &member).await?;
    check_shared(&member_dir, rotated)?;
    publish(&root, None).await?;
    latest(&root, &member).await?;
    assert!(matches!(
        fleet_store::project_credential(&member_dir, SHARED)?,
        SharedCredential::Missing
    ));
    publish(&root, Some(rotated)).await?;
    latest(&root, &member).await?;
    root.revoke(*member_id.as_bytes()).await?;
    member
        .adopt_revocation(root.public_snapshot().await)
        .await?;
    assert!(matches!(
        fleet_store::project_credential(&member_dir, SHARED)?,
        SharedCredential::Missing
    ));

    damaged_membership(&data, &member_dir)?;
    std::fs::remove_dir_all(path)?;
    Ok(())
}

fn damaged_membership(data: &Path, member_dir: &Path) -> anyhow::Result<()> {
    let identity_path = data.join("host.key");
    std::fs::copy(member_dir.join("host.key"), &identity_path)?;
    assert!(fleet_store::project_credential(data, SHARED).is_err());
    std::fs::remove_file(identity_path)?;
    assert!(fleet_store::project_credential(data, SHARED).is_err());
    assert!(!data.join("host.key").exists());
    let sealed = member_dir.join("fleet.sealed");
    std::fs::write(&sealed, b"corrupt")?;
    assert!(fleet_store::project_credential(member_dir, SHARED).is_err());
    std::fs::remove_file(sealed)?;
    assert!(fleet_store::project_credential(member_dir, SHARED).is_err());
    Ok(())
}
