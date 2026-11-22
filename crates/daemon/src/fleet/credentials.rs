use std::collections::BTreeSet;

pub use host::fleet::Sharing;
use host::{fleet, serve::Configure};
use proto::{
    CommandId, CommandResult, Instance, InstanceId, Rejection, Roster,
};

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
    if let Some(fleet) = membership
        && let Err(reason) =
            update_fleet(registry, fleet, plugins, &command).await?
    {
        return Ok(CommandResult::Rejected(reason));
    }
    registry.configure(command, id).await
}

async fn update_fleet(
    registry: &Registry,
    fleet: &Membership,
    plugins: &Plugins,
    command: &Configure,
) -> Result<Result<(), Rejection>, Stopped> {
    match command {
        Configure::SetCredential {
            provider,
            credential,
        } => {
            let sharing = plugins.sharing(*provider, credential);
            share(registry, fleet, *provider, credential, sharing).await
        }
        Configure::Remove(id) => Ok(remove(fleet, id).await),
    }
}

async fn share(
    registry: &Registry,
    fleet: &Membership,
    provider: proto::Provider,
    credential: &proto::Credential,
    sharing: Sharing,
) -> Result<Result<(), Rejection>, Stopped> {
    if sharing == Sharing::Local {
        return Ok(Ok(()));
    }
    let (_, catalog) = registry.instances().await?;
    let resolved_account = catalog
        .of(provider)
        .ok_or(Rejection::FleetUnavailable)
        .and_then(|profile| {
            profile
                .account(credential)
                .map_err(|reason| Rejection::InvalidCredential { reason })
        });
    let result = match resolved_account {
        Ok(account) => {
            share_credential(
                fleet,
                InstanceId { provider, account },
                credential,
                sharing,
            )
            .await
        }
        Err(rejection) => Err(rejection),
    };
    Ok(result)
}

pub async fn share_credential(
    fleet: &Membership,
    id: InstanceId,
    credential: &proto::Credential,
    sharing: Sharing,
) -> Result<(), Rejection> {
    if sharing == Sharing::Local {
        return Ok(());
    }
    fleet
        .change_credentials(|roster| {
            host::fleet::store(roster, id, credential, sharing);
        })
        .await
        .map_err(|_| Rejection::FleetUnavailable)
}

async fn remove(
    fleet: &Membership,
    instance: &InstanceId,
) -> Result<(), Rejection> {
    if fleet.credentials().await.of(instance).is_none() {
        return Ok(());
    }
    fleet
        .change_credentials(|roster| {
            roster.0.remove(instance);
        })
        .await
        .map_err(|_| Rejection::FleetUnavailable)
}

pub fn follow_credentials(
    registry: Registry,
    slot: super::Slot,
    plugins: Plugins,
    log: crate::log::Log,
) {
    tokio::spawn(async move {
        let mut observed = slot.subscribe();
        let joined = tokio::select! {
            result = observed.wait_for(Option::is_some) => result.ok().and_then(|mode| mode.clone()),
            () = log.stopped() => None,
        };
        let Some(fleet) = joined else {
            return;
        };
        reconcile(&registry, &fleet, &plugins, &log).await;
    });
}

async fn reconcile(
    registry: &Registry,
    fleet: &Membership,
    plugins: &Plugins,
    log: &crate::log::Log,
) {
    let mut versions = fleet.versions();
    let mut applied = Roster::default();
    loop {
        let mut known = fleet.applied().await;
        let current = active_credentials(fleet).await;
        apply(registry, plugins, &mut applied, &mut known, current).await;
        record(fleet, &known).await;
        if !wait(&mut versions, log).await {
            return;
        }
    }
}

async fn active_credentials(fleet: &Membership) -> Roster {
    if fleet.is_active().await {
        fleet.credentials().await
    } else {
        Roster::default()
    }
}

async fn record(fleet: &Membership, applied: &BTreeSet<InstanceId>) {
    if fleet.record_applied(applied).await.is_err() {
        tracing::warn!("could not record applied fleet credentials");
    }
}

async fn wait(
    versions: &mut tokio::sync::watch::Receiver<u64>,
    log: &crate::log::Log,
) -> bool {
    tokio::select! {
        result = versions.changed() => result.is_ok(),
        () = tokio::time::sleep(std::time::Duration::from_secs(30)) => true,
        () = log.stopped() => false,
    }
}

async fn apply(
    registry: &Registry,
    plugins: &Plugins,
    applied: &mut Roster,
    known: &mut BTreeSet<InstanceId>,
    current: Roster,
) {
    let removals = fleet::removal_plan(known, &current)
        .cloned()
        .collect::<Vec<_>>();
    remove_retired(registry, applied, known, removals).await;
    for (id, instance) in current.0 {
        if configure_changed(registry, plugins, applied, &id, &instance).await {
            known.insert(id.clone());
            applied.0.insert(id, instance);
        }
    }
}

async fn remove_retired(
    registry: &Registry,
    applied: &mut Roster,
    known: &mut BTreeSet<InstanceId>,
    removals: Vec<InstanceId>,
) {
    for id in removals {
        if perform(registry, Configure::Remove(id.clone())).await {
            applied.0.remove(&id);
            known.remove(&id);
        }
    }
}

async fn configure_changed(
    registry: &Registry,
    plugins: &Plugins,
    applied: &Roster,
    id: &InstanceId,
    instance: &Instance,
) -> bool {
    if !fleet::wants_update(applied, id, instance) {
        return false;
    }
    let Some(credential) = instance.credential.as_ref().filter(|credential| {
        plugins.sharing(id.provider, credential) != Sharing::Local
    }) else {
        return false;
    };
    perform(
        registry,
        Configure::SetCredential {
            provider: id.provider,
            credential: credential.clone(),
        },
    )
    .await
}

async fn perform(registry: &Registry, command: Configure) -> bool {
    let mut bytes = [0; 16];
    if getrandom::fill(&mut bytes).is_err() {
        return false;
    }
    registry
        .configure(command.clone(), CommandId(u128::from_le_bytes(bytes)))
        .await
        .is_ok_and(|result| fleet::applied(&command, &result))
}
