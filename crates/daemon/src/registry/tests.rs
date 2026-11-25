use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use agents::runtime::{Command, Event as AgentEvent};
use agents::{Account, Credential, Models, Provider};
use host::log::Epoch;
use host::registry::{
    Availability, InstanceCommand, InstanceId, InstanceRefusal, InstanceResult,
    Roster,
};
use host::registry::{OwnSignIn, Stored};
use host::serve::Relaunch;
use host::session::{SessionId, SessionRefusal, SessionResult};
use host::sign_in::SignInStep;
use host::sign_in::{Effect as SignInEffect, Key};
use link::CommandId;
use runtime::{Configuring, ProviderInstance, Runtime};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use super::{
    Core, Launcher, Log, RegistryActor, RegistryMsg, Reply, Report, Started,
    Store,
};
use crate::home::{HomeClone, Homing};
use crate::start::{Plugins, Providers};

const DEADLINE: Duration = Duration::from_secs(2);
const SESSION: SessionId = SessionId(1);

struct Data(PathBuf);

impl Data {
    fn new() -> std::io::Result<Self> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp")
            .join(format!("registry-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}

impl Drop for Data {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug)]
struct Pending {
    preparing: mpsc::UnboundedSender<()>,
    release: watch::Receiver<bool>,
}

impl ProviderInstance for Pending {
    fn configure(&self, _: Option<Credential>) -> Configuring<'_> {
        Box::pin(std::future::pending())
    }

    fn start(self: Arc<Self>) -> Runtime {
        let (commands, mut incoming) = mpsc::channel::<Command>(1);
        let (reported, events) = mpsc::channel(1);
        let mut release = self.release.clone();
        tokio::spawn(async move {
            if let Some(command) = incoming.recv().await {
                let _ = self.preparing.send(());
                if release.wait_for(|ready| *ready).await.is_ok() {
                    let _ = reported
                        .send(AgentEvent::Outcome(
                            command.failed("controlled launch failure"),
                        ))
                        .await;
                }
            }
        });
        Runtime { commands, events }
    }
}

struct Fixture {
    actor: RegistryActor,
    source: InstanceId,
    destination: InstanceId,
    preparing: mpsc::UnboundedReceiver<()>,
    release: watch::Sender<bool>,
    _data: Data,
}

impl Fixture {
    fn new() -> anyhow::Result<Self> {
        let data = Data::new()?;
        let source = InstanceId::named(Provider::new("claude"), "source");
        let destination =
            InstanceId::named(Provider::new("claude"), "destination");
        let providers = Providers::new(
            Plugins::default(),
            data.0.clone(),
            watch::channel(None).0,
        );
        let catalog = providers.catalog();
        let (store, _) = Store::open(&data.0, catalog.clone())?;
        let mut roster = Roster::default();
        for instance in [&source, &destination] {
            roster
                .0
                .insert(instance.clone(), host::registry::Instance::default());
            store.save_change(
                &roster,
                &super::Change::Added(instance.clone()),
                None,
                None,
            )?;
        }
        let mut core = Core::new(&Stored::Opened(roster), catalog);
        let (began, preparing) = mpsc::unbounded_channel();
        let (release, released) = watch::channel(false);
        let adapter = Arc::new(Pending {
            preparing: began,
            release: released,
        });
        for starting in core.boot() {
            let _ = core.started(Started {
                id: starting.id,
                instance: starting.instance,
                launcher: Launcher::Running {
                    start: adapter.clone(),
                    own: OwnSignIn::Unmanaged,
                    chosen: None,
                    models: Ok(Models::none()),
                },
            });
        }
        let log = Log::spawn(String::from("registry fixture"), Epoch(1));
        let home = HomeClone::spawn(Homing {
            host: String::from("registry fixture"),
            data_dir: data.0.clone(),
            remote: None,
            roots: Vec::new(),
            user: None,
        });
        let (initialized, _) = watch::channel(false);
        let mut actor =
            RegistryActor::new(log, store, core, providers, home, initialized);
        let session = super::Session::restore(
            SESSION,
            host::session::SessionDirectory(
                data.0.to_string_lossy().into_owned(),
            ),
            host::session::Choice {
                model: None,
                effort: None,
            },
            actor.told(&Provider::new("claude")),
            (None, None),
            &[],
            actor.log.clone(),
        );
        actor.core.admit(SESSION, source.clone(), session);
        Ok(Self {
            actor,
            source,
            destination,
            preparing,
            release,
            _data: data,
        })
    }

    async fn launch(
        &mut self,
    ) -> anyhow::Result<JoinHandle<Result<SessionResult, crate::Stopped>>> {
        let Ok((session, adapter, offered, reservation)) =
            self.adapter().await?
        else {
            anyhow::bail!("could not obtain a relaunch reservation");
        };
        let launch = tokio::spawn(async move {
            session
                .relaunch(
                    Relaunch::Continue.order(offered),
                    adapter,
                    reservation,
                )
                .await
        });
        anyhow::ensure!(
            tokio::time::timeout(DEADLINE, self.preparing.recv())
                .await?
                .is_some(),
            "the relaunch did not reach the provider",
        );
        Ok(launch)
    }

    async fn adapter(
        &mut self,
    ) -> anyhow::Result<Result<super::Relaunch, SessionRefusal>> {
        let (reply, answer) = oneshot::channel();
        self.actor
            .on(RegistryMsg::Continue {
                session: SESSION,
                reply,
            })
            .await?;
        Ok(answer.await?)
    }

    async fn remove(
        &mut self,
        id: CommandId,
    ) -> anyhow::Result<InstanceResult> {
        let (reply, answer) = oneshot::channel();
        self.actor
            .on(RegistryMsg::Configure {
                configure: InstanceCommand::Remove {
                    instance: self.source.clone(),
                },
                id,
                reply,
            })
            .await?;
        Ok(answer.await?)
    }

    fn sign_in(&mut self) -> anyhow::Result<Key> {
        let lease = self.actor.store.signing(&self.source)?;
        let (_, effects) = self
            .actor
            .sign_ins
            .start(self.source.clone(), Some(lease.settings.clone()));
        self.actor.leases.insert(self.source.clone(), lease);
        effects
            .into_iter()
            .find_map(|effect| match effect {
                SignInEffect::Spawn { key, .. } => Some(key),
                _ => None,
            })
            .ok_or_else(|| anyhow::anyhow!("the sign-in did not start"))
    }

    async fn confirm(&mut self, key: Key) -> anyhow::Result<()> {
        self.actor
            .on(RegistryMsg::Run(
                key,
                Report::Checked(Ok(Some(self.destination.account.clone()))),
            ))
            .await?;
        Ok(())
    }
}

#[tokio::test]
async fn a_pending_relaunch_blocks_removal_then_removal_blocks_relaunch()
-> anyhow::Result<()> {
    let mut fx = Fixture::new()?;
    let launch = fx.launch().await?;
    assert_eq!(
        fx.remove(CommandId(uuid::Uuid::from_u128(1))).await?,
        InstanceResult::Refused(InstanceRefusal::InstanceInUse { sessions: 1 }),
    );
    fx.release.send_replace(true);
    let _ = tokio::time::timeout(DEADLINE, launch).await???;
    assert_eq!(
        fx.remove(CommandId(uuid::Uuid::from_u128(2))).await?,
        InstanceResult::Accepted
    );
    assert!(fx.actor.offers.borrow().of(&fx.source).is_none());
    assert!(matches!(
        fx.adapter().await?,
        Err(SessionRefusal::InstanceUnavailable { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn a_pending_relaunch_blocks_adoption_then_adoption_blocks_relaunch()
-> anyhow::Result<()> {
    let mut fx = Fixture::new()?;
    let launch = fx.launch().await?;
    let mut steps = fx.actor.live.subscribe();
    let first = fx.sign_in()?;
    fx.confirm(first).await?;
    let (key, result) = tokio::time::timeout(DEADLINE, fx.actor.runs.next())
        .await?
        .ok_or_else(|| anyhow::anyhow!("no adoption outcome"))?;
    fx.actor.on(RegistryMsg::Run(key, result)).await?;
    assert!(matches!(
        steps.try_recv()?.step,
        SignInStep::Failed { reason } if reason.contains("InstanceInUse")
    ));
    assert!(fx.actor.store.read_roster()?.of(&fx.source).is_some());

    fx.release.send_replace(true);
    let _ = tokio::time::timeout(DEADLINE, launch).await???;
    let second = fx.sign_in()?;
    fx.confirm(second).await?;
    assert!(fx.actor.store.read_roster()?.of(&fx.source).is_none());
    assert!(matches!(
        fx.adapter().await?,
        Err(SessionRefusal::InstanceUnavailable { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn a_cancelled_refresh_settles_its_availability_and_waiter()
-> anyhow::Result<()> {
    let mut fx = Fixture::new()?;
    let (channel, answer) = oneshot::channel();
    let effects = fx.actor.core.signed_in(
        fx.source.clone(),
        Some(Account::Named(String::from("source"))),
        &std::collections::BTreeSet::new(),
        Reply {
            removal: None,
            credential: None,
            channel,
        },
    );
    fx.actor.perform(effects);
    assert_eq!(
        fx.actor.offers.borrow().availability(&fx.source),
        Some(&Availability::Unavailable {
            reason: String::from("refreshing the instance's models"),
        }),
    );
    fx.actor.refreshes.abort_all();
    let registry = fx.actor.spawn();
    assert_eq!(
        tokio::time::timeout(DEADLINE, answer).await??,
        InstanceResult::Accepted
    );
    let (offers, _) =
        tokio::time::timeout(DEADLINE, registry.instances()).await??;
    assert_eq!(
        offers.availability(&fx.source),
        Some(&Availability::Unavailable {
            reason: String::from("the instance job was cancelled"),
        }),
    );
    assert!(matches!(
        offers.availability(&fx.destination),
        Some(Availability::Available { .. })
    ));
    let (current, _) =
        tokio::time::timeout(DEADLINE, registry.instances()).await??;
    assert_eq!(current, offers);
    Ok(())
}
