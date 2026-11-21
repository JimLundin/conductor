use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use iroh::endpoint::{Connection, RecvStream};
use iroh::protocol::Router;
use iroh::{Endpoint, SecretKey};
use net::wire;
use proto::fleet::{ApplicationState, Certificate, DeviceHello};
use proto::{
    Command, CommandBody, CommandId, CommandResult, Feed, HelloReply, HostSeq,
    InstanceId, Open, Provider, Subscribe,
};
use tokio::sync::{mpsc, watch};

use crate::credentials::Store;
use crate::home::{HomeClone, Homing};
use crate::log::Log;
use crate::registry::Registry;
use crate::serve::{Conductor, ConnectionOptions};
use crate::start::{Plugins, Providers};

#[derive(Debug)]
struct Controlled(runtime::Launch);

impl runtime::ProviderInstance for Controlled {
    fn configure(
        &self,
        credential: Option<proto::Credential>,
    ) -> runtime::Configuring<'_> {
        Box::pin(async move {
            match credential.as_ref().map(|values| values.value("token")) {
                Some("hold") => std::future::pending().await,
                Some(_) => Err(agents::ConfigurationFailure::Refused(
                    String::from("fixture refusal"),
                )),
                None => Ok(agents::Configuration {
                    named: None,
                    models: Ok(proto::Models::none()),
                }),
            }
        })
    }

    fn retire(&self, stage: agents::Retirement) -> runtime::Terminating<'_> {
        Box::pin(async move {
            if stage == agents::Retirement::Kill {
                std::fs::write(self.0.work.join("retired"), "yes")
                    .map_err(|error| error.to_string())?;
            }
            Ok(())
        })
    }

    fn start(self: Arc<Self>) -> runtime::Runtime {
        let (commands, _) = mpsc::channel(1);
        let (_, events) = mpsc::channel(1);
        runtime::Runtime { commands, events }
    }
}

async fn host(
    data: &Path,
    endpoint: &Endpoint,
    membership: &super::Membership,
) -> anyhow::Result<(Router, Log)> {
    let cli = data.join("version");
    std::fs::write(
        &cli,
        format!(
            "#!/bin/sh\nprintf '%s\\n' '{} (Claude Code)'\n",
            agents::pin::pins::CLAUDE_VERSION
        ),
    )?;
    std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700))?;
    let mut plugin = claude_cli::plugin(runtime::Settings {
        path: Some(cli),
        model: None,
    });
    plugin.instance = |launch| Arc::new(Controlled(launch));
    let plugins = Plugins::from_iter([plugin])
        .with_fleet_credentials(|_, _| host::fleet::Sharing::Shared);
    let slot = watch::channel(Some(membership.clone())).0;
    let providers =
        Providers::new(plugins.clone(), data.to_path_buf(), slot.clone());
    let (store, stored) = Store::open(data, providers.catalog())?;
    let log = Log::spawn(String::from("application fixture"), proto::Epoch(1));
    let home = HomeClone::spawn(Homing {
        host: String::from("fixture"),
        data_dir: data.to_path_buf(),
        remote: None,
        roots: Vec::new(),
    });
    let (registry, _) =
        Registry::spawn(log.clone(), store, &stored, providers, home.clone())
            .await?;
    crate::fleet::follow_credentials(
        registry.clone(),
        slot.clone(),
        plugins.clone(),
        log.clone(),
    );
    let conductor = Conductor::new(
        log.clone(),
        registry,
        home,
        ConnectionOptions {
            token: proto::AuthToken::from_bytes([0; 16]),
            push: None,
            fleet: slot,
            endpoint: endpoint.clone(),
            plugins,
        },
    );
    let router = Router::builder(endpoint.clone())
        .accept(proto::ALPN, conductor)
        .accept(
            proto::fleet::ALPN,
            super::Protocol::for_member(membership.clone()),
        )
        .spawn();
    Ok((router, log))
}

async fn subscribe(
    client: &Endpoint,
    key: &SecretKey,
    server: &Endpoint,
    certificate: &Certificate,
) -> anyhow::Result<(Connection, RecvStream)> {
    let connection = client.connect(server.addr(), proto::ALPN).await?;
    let proof = proto::fleet::device_proof(
        &certificate.identity.fleet,
        server.id().as_bytes(),
        client.id().as_bytes(),
    );
    let (mut send, mut recv) = connection.open_bi().await?;
    wire::write(
        &mut send,
        &Open::FleetHello(DeviceHello {
            certificate: certificate.clone(),
            proof: key.sign(&proof).to_bytes().to_vec(),
        }),
    )
    .await?;
    send.finish()?;
    let Some(HelloReply::Welcome(welcome)) = wire::read(&mut recv).await?
    else {
        anyhow::bail!("fixture hello refused");
    };
    let (mut send, feed) = connection.open_bi().await?;
    wire::write(
        &mut send,
        &Open::Subscribe(Subscribe {
            epoch: welcome.epoch,
            after_seq: HostSeq(0),
        }),
    )
    .await?;
    send.finish()?;
    Ok((connection, feed))
}

async fn command(
    connection: &Connection,
    command: Command,
) -> anyhow::Result<CommandResult> {
    tokio::time::timeout(Duration::from_secs(2), async {
        let (mut send, mut recv) = connection.open_bi().await?;
        wire::write(&mut send, &Open::Command(command)).await?;
        send.finish()?;
        wire::read(&mut recv)
            .await?
            .context("command stream closed")
    })
    .await?
}

async fn applied(
    feed: &mut RecvStream,
    version: u64,
    instance: &InstanceId,
    state: Option<ApplicationState>,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let frame =
                wire::read::<Feed>(feed).await?.context("feed closed")?;
            if let Feed::Configuration(Some(application)) = frame
                && application.desired == version
                && application.adopted == Some(version)
                && application.instances.get(instance) == state.as_ref()
            {
                return Ok(());
            }
        }
    })
    .await?
}

fn credential(value: &str) -> CommandBody {
    CommandBody::SetCredential {
        provider: Provider::Claude,
        credential: proto::Credential {
            kind: String::from("gateway"),
            values: [
                (
                    String::from("base_url"),
                    String::from("https://fixture.invalid"),
                ),
                (String::from("token"), String::from(value)),
            ]
            .into(),
        },
    }
}

#[tokio::test]
async fn two_hosts_report_acceptance_application_and_stuck_instance_retirement()
-> anyhow::Result<()> {
    let directory = super::dir()?;
    let root_endpoint = super::endpoint(41).await?;
    let member_endpoint = super::endpoint(42).await?;
    let client = super::endpoint(43).await?;
    let owner = SecretKey::from_bytes(&[43; 32]);
    let (root, member) = super::test_pair(
        &directory.join("root"),
        &SecretKey::from_bytes(&[41; 32]),
        &directory.join("member"),
        member_endpoint.id(),
    )
    .await?;
    let certificate = root
        .sign_owner(*client.id().as_bytes(), String::from("fixture owner"))
        .await?;
    let (root_router, root_log) =
        host(&directory.join("root"), &root_endpoint, &root).await?;
    let (member_router, member_log) =
        host(&directory.join("member"), &member_endpoint, &member).await?;
    let (connection, mut root_feed) =
        subscribe(&client, &owner, &root_endpoint, &certificate).await?;
    let (_member_connection, mut member_feed) =
        subscribe(&client, &owner, &member_endpoint, &certificate).await?;
    let instance =
        InstanceId::named(Provider::Claude, "https://fixture.invalid");
    let install = Command {
        id: CommandId(81),
        body: credential("hold"),
    };
    assert_eq!(
        command(&connection, install.clone()).await?,
        CommandResult::Accepted
    );
    let installed = root.snapshot().await.version;
    super::sync(&member_endpoint, &member, &root_endpoint).await?;
    applied(
        &mut root_feed,
        installed,
        &instance,
        Some(ApplicationState::Applying),
    )
    .await?;
    applied(
        &mut member_feed,
        installed,
        &instance,
        Some(ApplicationState::Applying),
    )
    .await?;
    assert_eq!(
        command(
            &connection,
            Command {
                id: CommandId(82),
                body: CommandBody::RemoveInstance {
                    instance: instance.clone()
                }
            }
        )
        .await?,
        CommandResult::Accepted
    );
    let removed = root.snapshot().await.version;
    super::sync(&member_endpoint, &member, &root_endpoint).await?;
    applied(&mut root_feed, removed, &instance, None).await?;
    applied(&mut member_feed, removed, &instance, None).await?;
    for name in ["root", "member"] {
        assert!(
            directory
                .join(name)
                .join("work")
                .join(host::registry::dir(&instance))
                .join("retired")
                .is_file()
        );
    }
    assert_eq!(
        command(&connection, install).await?,
        CommandResult::Accepted
    );
    assert_eq!(root.snapshot().await.version, removed);
    refused(
        &connection,
        (&root, &root_endpoint),
        (&member, &member_endpoint),
        &mut root_feed,
        &mut member_feed,
        &instance,
    )
    .await?;
    client.close().await;
    root_router.shutdown().await?;
    member_router.shutdown().await?;
    root_log.shutdown().await?;
    member_log.shutdown().await?;
    std::fs::remove_dir_all(directory)?;
    Ok(())
}

async fn refused(
    connection: &Connection,
    root_host: (&super::Membership, &Endpoint),
    member_host: (&super::Membership, &Endpoint),
    root_feed: &mut RecvStream,
    member_feed: &mut RecvStream,
    instance: &InstanceId,
) -> anyhow::Result<()> {
    let (root, root_endpoint) = root_host;
    let (member, member_endpoint) = member_host;
    assert_eq!(
        command(
            connection,
            Command {
                id: CommandId(83),
                body: credential("refused")
            }
        )
        .await?,
        CommandResult::Accepted
    );
    let refused = root.snapshot().await.version;
    super::sync(member_endpoint, member, root_endpoint).await?;
    applied(
        root_feed,
        refused,
        instance,
        Some(ApplicationState::Refused),
    )
    .await?;
    applied(
        member_feed,
        refused,
        instance,
        Some(ApplicationState::Refused),
    )
    .await?;
    Ok(())
}
