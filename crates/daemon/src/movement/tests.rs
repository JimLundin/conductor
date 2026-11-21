use std::net::{Ipv4Addr, SocketAddr};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use agents::Configuration;
use anyhow::{Context, Result};
use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh::{Endpoint, EndpointAddr, RelayMode, SecretKey};
use proto::movement::MoveId;
use proto::{
    ClientName, EventKind, HostPath, InstanceId, Models, Provider, SessionId,
};
use runtime::{
    Configuring, Instance, Launch, ProviderInstance, Runtime, Settings,
};
use tokio::sync::{mpsc, watch};

use crate::credentials::Store;
use crate::fleet::{Membership, Slot};
use crate::home::{HomeClone, Homing};
use crate::log::Log;
use crate::registry::Registry;
use crate::start::{Plugins, Providers};
use crate::workspace;

use super::{ALPN, Mover, Ownership};

const NATIVE: &str = "123e4567-e89b-12d3-a456-426614174000";
const TRANSCRIPT: &[u8] = b"{\"type\":\"user\",\"synthetic\":true}\n";
const PIN: &str = agents::pin::pins::CLAUDE_VERSION;

#[tokio::test]
async fn certified_loopback_move_keeps_the_source_until_a_durable_target_receipt()
-> Result<()> {
    let scratch = Temp::new()?;
    let source_dir = scratch.0.join("left");
    let target_dir = scratch.0.join("right");
    std::fs::create_dir(&source_dir)?;
    std::fs::create_dir(&target_dir)?;
    let source_key = SecretKey::from_bytes(&[13; 32]);
    let target_key = SecretKey::from_bytes(&[14; 32]);
    let (left, right) = crate::fleet::test_pair(
        &source_dir,
        &source_key,
        &target_dir,
        target_key.public(),
    )
    .await?;
    let id = SessionId(ulid::Ulid::new().into());
    let source_path = seed(&source_dir, id).await?;
    let source =
        Local::start(&source_dir, &source_key, left.clone(), PIN).await?;
    let wrong =
        Local::start(&target_dir, &target_key, right.clone(), "0.0.0").await?;
    let target_node = *wrong.mover.endpoint.id().as_bytes();
    let rejected = MoveId(1);
    source.mover.settle(id, target_node, rejected).await?;
    let refusal = tokio::time::timeout(
        Duration::from_secs(15),
        source.mover.transfer(wrong.address.clone(), id, rejected),
    )
    .await?
    .err()
    .context("a different CLI version must be refused")?;
    assert!(
        refusal.to_string().contains("pin"),
        "unexpected refusal: {refusal:#}"
    );
    source
        .mover
        .note_failure(id, target_node, rejected, refusal.to_string())
        .await?;
    assert_eq!(source.ownership(id).await?, Ownership::Local);
    assert_eq!(
        wrong.mover.log.finish_history(id).await?,
        [] as [proto::Event; 0]
    );
    wrong.stop().await?;

    let target =
        Local::start(&target_dir, &target_key, right.clone(), PIN).await?;
    let accepted = MoveId(2);
    source.mover.settle(id, target_node, accepted).await?;
    interrupt_before_bundle(&source, &target, id, accepted).await?;
    assert_eq!(
        target.mover.log.finish_history(id).await?,
        [] as [proto::Event; 0]
    );
    assert!(
        matches!(source.ownership(id).await?, Ownership::Moving { id: known, .. } if known == accepted)
    );
    tokio::time::timeout(
        Duration::from_secs(15),
        source.mover.transfer(target.address.clone(), id, accepted),
    )
    .await??;
    assert!(
        matches!(source.ownership(id).await?, Ownership::Moving { id: known, .. } if known == accepted)
    );
    assert!(source_path.is_dir());
    assert_eq!(std::fs::read(native_file(&source_dir, id))?, TRANSCRIPT);
    assert_target(&target, id).await?;
    source.stop().await?;
    target.stop().await?;

    let source = Local::start(&source_dir, &source_key, left, PIN).await?;
    let target = Local::start(&target_dir, &target_key, right, PIN).await?;
    let before = target.mover.log.finish_history(id).await?;
    source.mover.settle(id, target_node, accepted).await?;
    tokio::time::timeout(
        Duration::from_secs(15),
        source.mover.transfer(target.address.clone(), id, accepted),
    )
    .await??;
    assert_eq!(target.mover.log.finish_history(id).await?, before);
    source.mover.release(id, target_node, accepted).await?;
    source.mover.clean_released(id, accepted).await?;
    assert!(!source_path.try_exists()?);
    assert!(!native_file(&source_dir, id).try_exists()?);
    assert_target(&target, id).await?;
    source.stop().await?;
    target.stop().await?;
    Ok(())
}

async fn interrupt_before_bundle(
    source: &Local,
    target: &Local,
    id: SessionId,
    move_id: MoveId,
) -> Result<()> {
    let to = *target.mover.endpoint.id().as_bytes();
    let events = source.mover.log.finish_history(id).await?;
    let scratch = super::Scratch::new(&source.mover.data)?;
    let prepared =
        super::Prepared::of(&source.mover, id, move_id, events, scratch)
            .await?;
    let (_connection, mut send, mut recv) =
        source.mover.connect(target.address.clone()).await?;
    net::wire::write(&mut send, &super::Request::Transfer(prepared.header))
        .await?;
    let ready = super::required(&mut recv).await?;
    assert!(matches!(ready, super::Reply::Ready { .. }));
    source
        .mover
        .phase(id, to, move_id, proto::movement::Phase::SendingWorkspace)
        .await?;
    send.write_all(&32u64.to_be_bytes()).await?;
    send.write_all(b"partial").await?;
    send.finish()?;
    let super::Reply::Refused(reason) = super::required(&mut recv).await?
    else {
        anyhow::bail!("the target did not report the interrupted stream");
    };
    source.mover.note_failure(id, to, move_id, reason).await?;
    Ok(())
}

async fn assert_target(target: &Local, id: SessionId) -> Result<()> {
    let history = target.mover.log.finish_history(id).await?;
    let files = host::movement::SessionFiles::of(&history)
        .map_err(anyhow::Error::msg)?;
    let moved = Path::new(&files.workspace.worktree.0).join("moved.txt");
    assert_eq!(std::fs::read_to_string(moved)?, "working edit\n");
    assert_eq!(
        std::fs::read(native_file(&target.mover.data, id))?,
        TRANSCRIPT
    );
    assert!(target.mover.registry.find(id).await?.is_some());
    Ok(())
}

async fn seed(data: &Path, id: SessionId) -> Result<PathBuf> {
    let repo = data.join("project");
    std::fs::create_dir(&repo)?;
    let _ = workspace::text(&repo, &["init", "--quiet"]).await?;
    std::fs::write(repo.join("first.txt"), "tracked\n")?;
    let _ = workspace::text(&repo, &["add", "--", "first.txt"]).await?;
    let _ = workspace::text(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "seed",
        ],
    )
    .await?;
    let original = repo
        .to_str()
        .context("invalid synthetic repository path")?
        .to_owned();
    let workspace =
        workspace::create(data, &HostPath(original), id, None).await?;
    let path = PathBuf::from(&workspace.worktree.0);
    std::fs::write(path.join("moved.txt"), "working edit\n")?;
    let transcript = native_file(data, id);
    std::fs::create_dir_all(
        transcript
            .parent()
            .context("missing transcript directory")?,
    )?;
    std::fs::write(&transcript, TRANSCRIPT)?;
    let log = Log::open(String::from("source"), &data.join("host.redb"))?;
    log.append(
        id,
        EventKind::SessionCreated {
            instance: InstanceId::own(Provider::Claude),
            provider: Provider::Claude,
            directory: workspace.original.clone(),
            by: ClientName(String::from("test")),
            model: None,
            effort: None,
        },
    )
    .await?;
    log.append(id, EventKind::WorkspaceCreated { workspace })
        .await?;
    log.append(
        id,
        EventKind::UserMessage {
            text: String::from("synthetic turn"),
            by: ClientName(String::from("test")),
        },
    )
    .await?;
    log.append(
        id,
        EventKind::TurnCompleted {
            outcome: proto::TurnOutcome::Completed,
        },
    )
    .await?;
    log.conversation(id, String::from(NATIVE)).await?;
    log.shutdown().await?;
    log.stopped().await;
    Ok(path)
}

fn native_file(data: &Path, id: SessionId) -> PathBuf {
    let account = host::registry::dir(&InstanceId::own(Provider::Claude));
    data.join("home")
        .join(account)
        .join("projects")
        .join(ulid::Ulid::from(id.0).to_string())
        .join(format!("{NATIVE}.jsonl"))
}

struct Local {
    mover: Mover,
    router: Router,
    address: EndpointAddr,
}

impl Local {
    async fn start(
        data: &Path,
        key: &SecretKey,
        membership: Membership,
        version: &str,
    ) -> Result<Self> {
        let endpoint = Endpoint::builder(presets::Minimal)
            .relay_mode(RelayMode::Disabled)
            .secret_key(key.clone())
            .bind()
            .await?;
        let slot = watch::channel(Some(membership)).0;
        let mover = local_mover(data, endpoint.clone(), slot, version).await?;
        let ipv4 = endpoint
            .addr()
            .ip_addrs()
            .find_map(|address| {
                address.is_ipv4().then(|| {
                    SocketAddr::from((Ipv4Addr::LOCALHOST, address.port()))
                })
            })
            .context("loopback endpoint has no IPv4 port")?;
        let address = EndpointAddr::new(endpoint.id()).with_ip_addr(ipv4);
        let router = Router::builder(endpoint)
            .accept(ALPN, mover.clone())
            .spawn();
        Ok(Self {
            mover,
            router,
            address,
        })
    }

    async fn ownership(&self, session: SessionId) -> Result<Ownership> {
        Ok(Ownership::replay(
            &self.mover.log.finish_history(session).await?,
        ))
    }

    async fn stop(self) -> Result<()> {
        self.router.shutdown().await?;
        self.mover.log.shutdown().await?;
        self.mover.log.stopped().await;
        Ok(())
    }
}

async fn local_mover(
    data: &Path,
    endpoint: Endpoint,
    slot: Slot,
    version: &str,
) -> Result<Mover> {
    let cli = data.join("fixture-version");
    std::fs::write(
        &cli,
        format!("#!/bin/sh\nprintf '%s\\n' '{version} (Claude Code)'\n"),
    )?;
    std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700))?;
    let mut plugin = claude_cli::plugin(Settings {
        path: Some(cli),
        model: None,
    });
    plugin.instance = fixture_instance;
    let plugins: Plugins = std::iter::once(plugin).collect();
    let providers = Providers::new(plugins, data.to_path_buf(), slot.clone());
    let (store, stored) = Store::open(data, providers.catalog())?;
    let log = Log::open(String::from("fixture"), &data.join("host.redb"))?;
    let home = HomeClone::spawn(Homing {
        host: String::from("fixture"),
        data_dir: data.to_path_buf(),
        remote: None,
        roots: Vec::new(),
    });
    let (registry, mut ready) =
        Registry::spawn(log.clone(), store, &stored, providers, home.clone())
            .await?;
    drop(
        tokio::time::timeout(
            Duration::from_secs(15),
            ready.wait_for(|initialized| *initialized),
        )
        .await??,
    );
    Ok(Mover::new(
        endpoint,
        slot,
        log,
        registry,
        home,
        data.to_path_buf(),
    ))
}

#[derive(Debug)]
struct FakeProvider;

fn fixture_instance(_: Launch) -> Instance {
    Arc::new(FakeProvider)
}

impl ProviderInstance for FakeProvider {
    fn configure(&self, _: Option<proto::Credential>) -> Configuring<'_> {
        Box::pin(async {
            Ok(Configuration {
                named: None,
                models: Ok(Models::none()),
            })
        })
    }

    fn start(self: Arc<Self>) -> Runtime {
        let (commands, _) = mpsc::channel(1);
        let (_, events) = mpsc::channel(1);
        Runtime { commands, events }
    }
}

struct Temp(PathBuf);

impl Temp {
    fn new() -> std::io::Result<Self> {
        let target =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        std::fs::create_dir_all(&target)?;
        let dir = target
            .join(format!("conductor-move-loopback-{}", ulid::Ulid::new()));
        std::fs::create_dir(&dir)?;
        Ok(Self(dir))
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
