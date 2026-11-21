pub use host::fleet::Sharing;
use host::{fleet, serve::Configure};
use proto::{CommandId, CommandResult, InstanceId};

use super::Membership;
use crate::Stopped;
use crate::registry::Registry;
use crate::start::Plugins;

pub async fn configure(
    registry: &Registry,
    membership: Option<&Membership>,
    plugins: &Plugins,
    command: Configure,
    id: CommandId,
) -> Result<CommandResult, Stopped> {
    let Some(fleet) = membership else {
        return registry.configure(command, id).await;
    };
    configure_shared(registry, fleet, plugins, command, id).await
}

async fn configure_shared(
    registry: &Registry,
    fleet: &Membership,
    plugins: &Plugins,
    command: Configure,
    id: CommandId,
) -> Result<CommandResult, Stopped> {
    if fleet.accepted(id).await? {
        return Ok(CommandResult::Accepted);
    }
    let (_, catalog) = registry.instances().await?;
    let sharing = command
        .credential()
        .map_or(Sharing::Shared, |(provider, credential)| {
            plugins.sharing(provider, credential)
        });
    match fleet::acceptance::admit(
        &command,
        &catalog,
        &fleet.credentials().await,
        sharing,
    ) {
        Ok(Some(mutation)) => fleet.accept(id, &mutation).await,
        Ok(None) => registry.configure(command, id).await,
        Err(reason) => Ok(CommandResult::Rejected(reason)),
    }
}

pub async fn share_credential(
    fleet: &Membership,
    command: CommandId,
    id: InstanceId,
    credential: &proto::Credential,
    sharing: Sharing,
) -> Result<CommandResult, Stopped> {
    fleet
        .accept(
            command,
            &fleet::acceptance::Mutation::Set {
                instance: id,
                credential: credential.clone(),
                sharing,
            },
        )
        .await
}

pub fn follow_credentials(
    registry: Registry,
    slot: super::Slot,
    plugins: Plugins,
    log: crate::log::Log,
) {
    let initially_enrolled = slot.borrow().is_some();
    tokio::spawn(async move {
        let mut observed = slot.subscribe();
        let joined = tokio::select! {
            result = observed.wait_for(Option::is_some) => result.ok().and_then(|mode| mode.clone()),
            () = log.stopped() => None,
        };
        let Some(fleet) = joined else {
            return;
        };
        let revision = initially_enrolled.then_some(fleet.revision().await);
        reconcile(&registry, &fleet, &plugins, &log, revision).await;
    });
}

async fn reconcile(
    registry: &Registry,
    fleet: &Membership,
    plugins: &Plugins,
    log: &crate::log::Log,
    revision: Option<proto::fleet::Revision>,
) {
    let mut versions = fleet.versions();
    let mut core =
        host::registry::reconciliation::Reconciliation::new(revision);
    let began = tokio::time::Instant::now();
    loop {
        let _ = apply(registry, fleet, plugins, &mut core, began).await;
        if !wait(&mut versions, log).await {
            return;
        }
    }
}

async fn apply(
    registry: &Registry,
    fleet: &Membership,
    plugins: &Plugins,
    core: &mut host::registry::reconciliation::Reconciliation,
    began: tokio::time::Instant,
) -> Result<(), Stopped> {
    let mut desired = fleet.desired().await?;
    desired.credentials.0.retain(|id, settings| {
        settings.credential.as_ref().is_some_and(|credential| {
            plugins.sharing(id.provider, credential) != Sharing::Local
        })
    });
    let version = desired.version;
    registry
        .desire(version, desired.credentials.clone(), desired.owned.clone())
        .await?;
    core.desire(desired);
    registry.reconfigure_fleet(core.changed_providers()).await?;
    core.reconfigured();
    for work in core.plan(began.elapsed().as_secs()) {
        let command = command_id().map_err(|_| Stopped {})?;
        let result = registry.apply(work.command(), command).await?;
        core.completed(work, &result, began.elapsed().as_secs());
    }
    if core.settled() {
        registry.adopted(version).await?;
    }
    fleet
        .record_applied(version, core.owned())
        .await
        .map_err(|_| {
            tracing::warn!("could not record adopted fleet credentials");
            Stopped {}
        })
}

fn command_id() -> Result<CommandId, getrandom::Error> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes)?;
    Ok(CommandId(u128::from_le_bytes(bytes)))
}

async fn wait(
    versions: &mut tokio::sync::watch::Receiver<u64>,
    log: &crate::log::Log,
) -> bool {
    tokio::select! {
        result = versions.changed() => result.is_ok(),
        () = tokio::time::sleep(std::time::Duration::from_secs(1)) => true,
        () = log.stopped() => false,
    }
}
