use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::credentials::Store;
use crate::home::{HomeClone, Homing};
use crate::log::{Log, TransportStopped};
use crate::registry::{Launcher, Registry};
use crate::serve::Conductor;
use crate::sign_in::Signing;
use agents::Provider;
use anyhow::Context;
use home_files::FolderStore;
use host::registry::{AccountAdoption, sign_in_instance};
use host::registry::{Catalog, Instance, InstanceId};
use host::start::{Mode, Next, Output};
use install::Home;
use install::process::command;
use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh::{Endpoint, RelayMode};
use link::Ticket;
use qrcode::render::svg;
use qrcode::render::unicode::Dense1x2;
use runtime::{Launch, Plugin, Settings};
use tracing::Instrument;

#[derive(Debug, Clone, Default)]
pub struct Plugins(
    Arc<BTreeMap<Provider, Plugin>>,
    Option<fn(&Provider, &agents::Credential) -> crate::fleet::Sharing>,
);

impl FromIterator<Plugin> for Plugins {
    fn from_iter<I: IntoIterator<Item = Plugin>>(plugins: I) -> Self {
        Self(
            Arc::new(
                plugins
                    .into_iter()
                    .map(|plugin| (plugin.profile.provider.clone(), plugin))
                    .collect(),
            ),
            None,
        )
    }
}

impl Plugins {
    #[must_use]
    pub fn with_fleet_credentials(
        mut self,
        policy: fn(&Provider, &agents::Credential) -> crate::fleet::Sharing,
    ) -> Self {
        self.1 = Some(policy);
        self
    }

    pub(crate) fn sharing(
        &self,
        provider: &Provider,
        credential: &agents::Credential,
    ) -> crate::fleet::Sharing {
        self.1.map_or(crate::fleet::Sharing::Local, |policy| {
            policy(provider, credential)
        })
    }

    #[must_use]
    fn catalog(&self) -> Catalog {
        Catalog(
            self.0
                .iter()
                .map(|(provider, plugin)| {
                    (provider.clone(), plugin.profile.info.clone())
                })
                .collect(),
        )
    }
}

#[derive(Debug, Clone)]
pub struct Providers {
    plugins: Plugins,
    data_dir: PathBuf,
    fleet: crate::fleet::Slot,
}

impl Providers {
    pub(crate) const fn new(
        plugins: Plugins,
        data_dir: PathBuf,
        fleet: crate::fleet::Slot,
    ) -> Self {
        Self {
            plugins,
            data_dir,
            fleet,
        }
    }

    pub fn catalog(&self) -> Catalog {
        self.plugins.catalog()
    }

    pub fn instructions_file(&self, provider: &Provider) -> Option<String> {
        self.plugins
            .0
            .get(provider)
            .map(|plugin| plugin.profile.instructions_file.clone())
    }

    pub async fn launch(
        &self,
        id: &InstanceId,
        instance: Instance,
        mode: Mode,
    ) -> Launcher {
        let span = tracing::info_span!("instance", id = %id);
        match prepare(&self.plugins, &self.data_dir, id, &instance) {
            Ok(prepared) => {
                run(prepared, &self.data_dir, instance, mode)
                    .instrument(span)
                    .await
            }
            Err(reason) => Launcher::Unavailable(reason),
        }
    }

    pub(crate) async fn share_credential(
        &self,
        id: InstanceId,
        credential: &agents::Credential,
    ) -> Result<(), host::registry::InstanceRefusal> {
        let membership = self.fleet.borrow().clone();
        if let Some(fleet) = membership {
            let sharing = self.plugins.sharing(&id.provider, credential);
            crate::fleet::share_credential(&fleet, id, credential, sharing)
                .await?;
        }
        Ok(())
    }

    pub(crate) async fn credential_sign_in(
        &self,
        id: &InstanceId,
        target: &host::sign_in::CredentialTarget,
        instance: &Instance,
        link: tokio::sync::oneshot::Sender<String>,
        code: tokio::sync::oneshot::Receiver<agents::SignInCode>,
    ) -> Option<agents::Credential> {
        let agents::Account::Named(label) = &target.destination.account else {
            return None;
        };
        let isolated = credential_home(&self.data_dir).ok()?;
        let prepared =
            prepare(&self.plugins, isolated.path(), id, instance).ok()?;
        let path = provider_program(
            prepared.settings.path,
            &prepared.plugin,
            &self.data_dir,
        )
        .await
        .ok()?;
        let runtime = (prepared.plugin.instance)(Launch {
            path,
            home: prepared.home,
            work: prepared.work,
        });
        runtime
            .credential_sign_in(&target.kind, label.clone(), link, code)?
            .await
            .ok()
    }

    pub async fn signing(
        &self,
        id: &InstanceId,
        instance: &Instance,
    ) -> Result<Signing, String> {
        let Prepared {
            plugin,
            settings,
            home,
            work: _,
        } = prepare(&self.plugins, &self.data_dir, id, instance)?;
        let program =
            provider_program(settings.path, &plugin, &self.data_dir).await?;
        let profile = plugin.profile;
        Ok(Signing {
            program,
            home,
            flow: profile.sign_in,
            check: profile.status,
            account: profile.account,
            read: plugin.account,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Hosting {
    pub data_dir: Option<PathBuf>,
    pub name: String,
    pub web_url: String,
    pub home: Option<String>,
    pub projects: Vec<PathBuf>,
}

pub async fn serve(
    hosting: Hosting,
    plugins: Plugins,
    workspaces: crate::Workspaces,
) -> anyhow::Result<()> {
    let Hosting {
        data_dir: given,
        name,
        web_url,
        home: remote,
        projects,
    } = hosting;
    let data_dir = data_dir_or_default(given)?;
    let endpoint = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Custom(net::n0_relays()))
        .secret_key(crate::load_or_create_key(&data_dir)?)
        .bind()
        .await?;
    endpoint.online().await;
    let hosted = start(
        endpoint,
        plugins,
        workspaces,
        HostOptions {
            data_dir: data_dir.clone(),
            name,
            home: remote,
            projects,
            user: home_files::user(),
        },
    )
    .await?;
    let announced = if hosted.has_fleet {
        writeln!(
            std::io::stdout().lock(),
            "Fleet member. Use orchestrator fleet status to inspect membership."
        )
        .map_err(anyhow::Error::from)
    } else {
        hosted
            .ticket()
            .and_then(|ticket| announce(&data_dir, &web_url, &ticket))
    };
    let ended = match announced {
        Ok(()) => tokio::select! {
            result = tokio::signal::ctrl_c() => result.map_err(anyhow::Error::from),
            () = hosted.log.stopped() => Err(anyhow::Error::from(crate::Stopped {})),
        },
        Err(error) => Err(error),
    };
    hosted.shutdown().await?;
    ended
}

#[derive(Debug, Clone)]
pub struct HostOptions {
    pub data_dir: PathBuf,
    pub name: String,
    pub home: Option<String>,
    pub projects: Vec<PathBuf>,
    pub user: Option<PathBuf>,
}

pub struct Hosted {
    router: Router,
    log: Log,
    token: link::AuthToken,
    initialized: tokio::sync::watch::Receiver<bool>,
    fleet_lock: std::fs::File,
    has_fleet: bool,
    operations: project_runtime::operations::Operations,
}

impl Hosted {
    #[must_use]
    pub const fn operations(&self) -> &project_runtime::operations::Operations {
        &self.operations
    }

    pub fn ticket(&self) -> anyhow::Result<Ticket> {
        let endpoint = self.router.endpoint();
        let relay = endpoint
            .addr()
            .relay_urls()
            .next()
            .cloned()
            .context("online, but no home relay")?;
        Ok(Ticket {
            host: *endpoint.id().as_bytes(),
            relay: relay.to_string(),
            token: self.token,
        })
    }

    pub async fn initialized(&self) -> anyhow::Result<()> {
        self.initialized
            .clone()
            .wait_for(|ready| *ready)
            .await
            .context("the registry stopped during bootstrap")?;
        Ok(())
    }

    pub async fn shutdown(self) -> anyhow::Result<()> {
        let Self {
            router,
            log,
            fleet_lock: _fleet_lock,
            ..
        } = self;
        let (transport, stopped) = TransportStopped::stop(router).await;
        log.shutdown(transport).await?;
        stopped?;
        Ok(())
    }
}

pub async fn start(
    endpoint: Endpoint,
    plugins: Plugins,
    workspaces: crate::Workspaces,
    options: HostOptions,
) -> anyhow::Result<Hosted> {
    let (lock, fleet) = open_fleet(&endpoint, &options.data_dir).await?;
    let has_fleet = fleet.is_some();
    let operations = project_runtime::operations::Operations::bind(
        &options.data_dir,
        *endpoint.id().as_bytes(),
    )?;
    let data = options.data_dir.clone();
    let (fleet_slot, _) = tokio::sync::watch::channel(fleet);
    let started = assemble(
        plugins,
        options,
        workspaces,
        fleet_slot.clone(),
        endpoint.clone(),
    )
    .await;
    match started {
        Ok((conductor, log, token, initialized)) => {
            let protocol = crate::fleet::Protocol::new(
                fleet_slot.clone(),
                data,
                endpoint.id(),
                host::fleet::EnrollmentToken::from_bytes(token.to_bytes()),
            );
            let router = Router::builder(endpoint.clone())
                .accept(link::ALPN, conductor)
                .accept(host::fleet::ALPN, protocol)
                .spawn();
            tokio::spawn(crate::fleet::follow_when_enrolled(
                endpoint, fleet_slot,
            ));
            Ok(Hosted {
                router,
                log,
                token,
                initialized,
                fleet_lock: lock,
                has_fleet,
                operations,
            })
        }
        Err(error) => {
            endpoint.close().await;
            Err(error)
        }
    }
}

async fn open_fleet(
    endpoint: &Endpoint,
    data_dir: &Path,
) -> anyhow::Result<(std::fs::File, Option<crate::fleet::Membership>)> {
    let opened = crate::fleet::exclusive(data_dir).and_then(|lock| {
        crate::fleet::Membership::open(data_dir, endpoint.id())
            .map(|fleet| (lock, fleet))
    });
    let (lock, fleet) = match opened {
        Ok(membership) => membership,
        Err(error) => {
            endpoint.close().await;
            return Err(error);
        }
    };
    if let Some(membership) = &fleet {
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(4),
            crate::fleet::refresh(endpoint, membership),
        )
        .await;
    }
    Ok((lock, fleet))
}

async fn assemble(
    plugins: Plugins,
    options: HostOptions,
    workspaces: crate::Workspaces,
    fleet: crate::fleet::Slot,
    endpoint: Endpoint,
) -> anyhow::Result<(
    Conductor,
    Log,
    link::AuthToken,
    tokio::sync::watch::Receiver<bool>,
)> {
    let HostOptions {
        data_dir,
        name,
        home: remote,
        projects,
        user,
    } = options;
    let token = crate::load_or_create_token(&data_dir)?;
    let (store, stored) = Store::open(&data_dir, plugins.catalog())?;
    let log = Log::open(name.clone(), &data_dir.join("conductor.redb"))?;
    let push =
        crate::push::Push::open(&data_dir, endpoint.id().as_bytes(), &log)
            .await
            .inspect_err(|_| tracing::warn!("could not initialize Web Push"))
            .ok();
    let home = HomeClone::spawn(Homing {
        host: name,
        data_dir: data_dir.clone(),
        remote,
        roots: projects,
        user,
        canvases: Box::new(FolderStore::new(crate::home::clone_of(&data_dir))),
    });
    let (registry, initialized) = Registry::spawn(
        log.clone(),
        store,
        &stored,
        Providers::new(plugins.clone(), data_dir, fleet.clone()),
        home.clone(),
        workspaces,
    )
    .await?;
    crate::fleet::follow_credentials(
        registry.clone(),
        fleet.clone(),
        plugins.clone(),
        log.clone(),
    );
    let conductor = Conductor::new(
        log.clone(),
        registry,
        home,
        crate::serve::ConnectionOptions {
            token,
            push,
            fleet,
            endpoint,
            plugins,
        },
    );
    Ok((conductor, log, token, initialized))
}

struct Prepared {
    plugin: Plugin,
    settings: Settings,
    home: Home,
    work: PathBuf,
}

fn prepare(
    plugins: &Plugins,
    data_dir: &Path,
    id: &InstanceId,
    instance: &Instance,
) -> Result<Prepared, String> {
    let plugin = plugins
        .0
        .get(&id.provider)
        .cloned()
        .ok_or_else(|| format!("no plugin runs {}", id.provider.name()))?;
    let home = private_home(data_dir, id, &plugin)?;
    let work = data_dir.join("work").join(host::registry::dir(id));
    std::fs::create_dir_all(&work)
        .map_err(|err| format!("creating {}: {err}", work.display()))?;
    let settings = Settings {
        path: instance
            .cli
            .as_ref()
            .map(|cli| PathBuf::from(&cli.0))
            .or_else(|| plugin.settings.path.clone()),
        model: instance
            .model
            .as_ref()
            .map(|model| model.0.clone())
            .or_else(|| plugin.settings.model.clone()),
    };
    Ok(Prepared {
        plugin,
        settings,
        home,
        work,
    })
}

async fn provider_program(
    configured: Option<PathBuf>,
    plugin: &Plugin,
    data_dir: &Path,
) -> Result<PathBuf, String> {
    match configured {
        Some(path) => Ok(path),
        None => install(plugin, data_dir).await,
    }
}

async fn install(plugin: &Plugin, data_dir: &Path) -> Result<PathBuf, String> {
    let pin = (plugin.pin)().map_err(|err| err.to_string())?;
    install::install(plugin.profile.provider.name(), pin, data_dir)
        .await
        .map_err(|err| err.to_string())
}

async fn run(
    prepared: Prepared,
    data_dir: &Path,
    instance: Instance,
    mode: Mode,
) -> Launcher {
    let Prepared {
        plugin,
        settings,
        home,
        work,
    } = prepared;
    let mut next = host::start::start(
        plugin.profile.provider.name(),
        settings.path,
        settings.model,
    );
    loop {
        next = match next {
            Next::Install(installing) => {
                installing.installed(install(&plugin, data_dir).await)
            }
            Next::Probe(path, probing) => {
                let start = (plugin.instance)(Launch {
                    path: path.clone(),
                    home: home.clone(),
                    work: work.clone(),
                });
                let program = path.display().to_string();
                let ran = version(command(
                    &path,
                    &home,
                    plugin.profile.version.clone(),
                ))
                .await;
                probing.probed(path, start, program, ran)
            }
            Next::CheckSignIn(path, checking) => checking.checked(
                &sign_in_check(command(
                    &path,
                    &home,
                    plugin.profile.status.clone(),
                ))
                .await
                .map(|(code, _)| code),
            ),
            Next::Configure(listing) => {
                let configured = listing
                    .instance()
                    .configure(instance.credential.clone())
                    .await;
                listing.configured(configured, mode)
            }
            Next::Done(launcher, notes) => {
                notes.iter().for_each(crate::log_note);
                break launcher;
            }
        }
    }
}

pub async fn login(
    given: Option<PathBuf>,
    plugins: Plugins,
    provider: Provider,
    account: Option<String>,
) -> anyhow::Result<()> {
    let data_dir = data_dir_or_default(given)?;
    let catalog = plugins.catalog();
    let (store, _) = Store::open(&data_dir, catalog)?;
    let requested = sign_in_instance(provider.clone(), account.as_deref());
    if let Some(to) = store.recovered_account(&requested) {
        tracing::info!(target: "host", instance = %to, "signed in");
        return Ok(());
    }
    let roster = store.read_roster()?;
    let instance =
        host::registry::picked(&roster, &provider, account.as_deref())
            .map_err(anyhow::Error::msg)?;
    let lease = store.signing(&instance)?;
    let key = crate::load_or_create_key(&data_dir)?;
    let membership = crate::fleet::Membership::open(&data_dir, key.public())?;
    let (fleet, _) = tokio::sync::watch::channel(membership);
    let providers = Providers::new(plugins, data_dir, fleet);
    let signing = providers
        .signing(&instance, &lease.settings)
        .await
        .map_err(anyhow::Error::msg)?;
    let status =
        command(&signing.program, &signing.home, signing.flow.run.clone())
            .status()
            .await?;
    let login =
        host::sign_in::Login::exited(status.code(), &status.to_string())
            .map_err(anyhow::Error::msg)?;
    let named = signing.confirm(login).await.map_err(anyhow::Error::msg)?;
    let Some(adoption) = AccountAdoption::named(&instance, named) else {
        return Ok(());
    };
    let to = adoption.to.clone();
    store.adopt(&lease, adoption)?;
    tracing::info!(target: "host", instance = %to, "signed in");
    Ok(())
}

fn credential_home(data_dir: &Path) -> std::io::Result<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("credential-sign-in-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir_in(data_dir)
}

fn private_home(
    data_dir: &Path,
    id: &InstanceId,
    plugin: &Plugin,
) -> Result<Home, String> {
    Home::private(data_dir, &host::registry::dir(id), &plugin.profile.config)
        .map_err(|err| format!("creating the private home for {id}: {err}"))
}

pub use project_runtime::data_dir_or_default;

const SIGN_IN_CHECK: std::time::Duration = std::time::Duration::from_secs(15);

pub async fn sign_in_check(
    mut status: tokio::process::Command,
) -> Result<(Option<i32>, String), String> {
    status.kill_on_drop(true);
    tokio::time::timeout(SIGN_IN_CHECK, status.output())
        .await
        .map_err(|elapsed| elapsed.to_string())?
        .map(|output| {
            (
                output.status.code(),
                String::from_utf8_lossy(&output.stdout).into_owned(),
            )
        })
        .map_err(|err| err.to_string())
}

async fn version(cli: tokio::process::Command) -> Result<Output, String> {
    version_within(cli, std::time::Duration::from_secs(15)).await
}

async fn version_within(
    mut cli: tokio::process::Command,
    deadline: std::time::Duration,
) -> Result<Output, String> {
    cli.kill_on_drop(true);
    tokio::time::timeout(deadline, cli.output())
        .await
        .map_err(|_| String::from("the CLI version probe timed out"))?
        .map(|output| Output {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        })
        .map_err(|err| err.to_string())
}

fn announce(
    data_dir: &Path,
    web_url: &str,
    ticket: &Ticket,
) -> anyhow::Result<()> {
    let text = ticket.to_text();
    let join = format!("{web_url}#join={text}");
    let mut out = std::io::stdout();
    writeln!(out, "data dir: {}", data_dir.display())?;
    writeln!(
        out,
        "host id:  {}",
        data_encoding::HEXLOWER.encode(&ticket.host)
    )?;
    writeln!(out, "relay:    {}", ticket.relay)?;
    writeln!(out, "ticket:   {text}")?;
    writeln!(out, "join:     {join}")?;
    match qrcode::QrCode::new(&join) {
        Ok(code) => {
            writeln!(out, "{}", dark_terminal_qr(&code))?;
            let svg_path = data_dir.join("join-qr.svg");
            let image =
                code.render::<svg::Color>().min_dimensions(512, 512).build();
            match crate::write_private(&svg_path, image.as_bytes()) {
                Ok(()) => writeln!(out, "qr image: {}", svg_path.display())?,
                Err(err) => tracing::warn!(
                    "could not write {}: {err}",
                    svg_path.display()
                ),
            }
        }
        Err(err) => {
            tracing::warn!(
                "could not render the join link as a QR code: {err}"
            );
        }
    }
    Ok(())
}

fn dark_terminal_qr(code: &qrcode::QrCode) -> String {
    code.render::<Dense1x2>()
        .dark_color(Dense1x2::Light)
        .light_color(Dense1x2::Dark)
        .quiet_zone(true)
        .build()
}

#[cfg(all(test, unix))]
mod version_tests {
    #[tokio::test]
    async fn a_pending_version_probe_times_out() {
        let mut cli = tokio::process::Command::new("sleep");
        cli.arg("3600");
        let result =
            super::version_within(cli, std::time::Duration::ZERO).await;
        assert_eq!(
            result.err().as_deref(),
            Some("the CLI version probe timed out")
        );
    }
}
