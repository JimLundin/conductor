//! `conductord`'s commands, for the plugins it is handed: start the host,
//! or sign an instance's CLI in.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::credentials::Store;
use crate::home::{HomeClone, Homing};
use crate::log::Log;
use crate::registry::{Launcher, Registry};
use crate::serve::Conductor;
use crate::sign_in::Signing;
use anyhow::Context;
use host::registry::{AccountAdoption, sign_in_instance};
use host::start::{Mode, Next, Output};
use install::Home;
use install::process::command;
use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh::{Endpoint, RelayMode, RelayUrl};
use net::Target;
use proto::{Catalog, Instance, InstanceId, Provider, Ticket};
use qrcode::render::svg;
use qrcode::render::unicode::Dense1x2;
use runtime::{Launch, Plugin, Settings};
use tracing::Instrument;

/// The providers a host runs, each by its plugin.
#[derive(Debug, Clone, Default)]
pub struct Plugins(
    Arc<BTreeMap<Provider, Plugin>>,
    Option<fn(&Provider, &proto::Credential) -> crate::fleet::Sharing>,
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
        policy: fn(&Provider, &proto::Credential) -> crate::fleet::Sharing,
    ) -> Self {
        self.1 = Some(policy);
        self
    }

    pub(crate) fn sharing(
        &self,
        provider: &Provider,
        credential: &proto::Credential,
    ) -> crate::fleet::Sharing {
        self.1.map_or(crate::fleet::Sharing::Local, |policy| {
            policy(provider, credential)
        })
    }

    /// How each provider presents itself, as its plugin says.
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
        credential: &proto::Credential,
    ) -> Result<(), proto::Rejection> {
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
        code: tokio::sync::oneshot::Receiver<proto::SignInCode>,
    ) -> Option<proto::Credential> {
        let proto::Account::Named(label) = &target.destination.account else {
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

/// How `conductord host` serves.
#[derive(Debug, Clone)]
pub struct Hosting {
    /// Where the host key lives; `None` takes the OS data dir for
    /// "conductor".
    pub data_dir: Option<PathBuf>,
    /// The name clients show for this host.
    pub name: String,
    /// The web app's URL; the printed join link is this plus
    /// `#join=<ticket>`.
    pub web_url: String,
    /// The home's remote; `None` keeps the one the host stored, if any.
    pub home: Option<String>,
    /// The directories the host looks for project checkouts in.
    pub projects: Vec<PathBuf>,
}

/// Serves this host's sessions over iroh with `plugins`' providers, and
/// prints a join ticket. It runs until Ctrl-C.
///
/// # Errors
///
/// Why the host could not start or stopped.
pub async fn serve(hosting: Hosting, plugins: Plugins) -> anyhow::Result<()> {
    let Hosting {
        data_dir: given,
        name,
        web_url,
        home: remote,
        projects,
    } = hosting;
    let data_dir = data_dir_or_default(given)?;
    let endpoint = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Custom(n0_relays()))
        .secret_key(crate::load_or_create_key(&data_dir)?)
        .bind()
        .await?;
    endpoint.online().await;
    let relay = endpoint
        .addr()
        .relay_urls()
        .next()
        .cloned()
        .context("online, but no home relay")?;
    let hosted = start(
        endpoint,
        plugins,
        HostOptions {
            data_dir: data_dir.clone(),
            name,
            home: remote,
            projects,
            user: home_files::user(),
        },
    )
    .await?;
    let target = hosted.target();
    let ticket = Ticket {
        host: *target.addr.id.as_bytes(),
        relay: relay.to_string(),
        token: target.token,
    };
    let announced = if hosted.has_fleet {
        writeln!(
            std::io::stdout().lock(),
            "Fleet member. Use conductord fleet status to inspect membership."
        )
        .map_err(anyhow::Error::from)
    } else {
        announce(&data_dir, &web_url, &ticket)
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

/// Settings for a host on an already bound endpoint.
#[derive(Debug, Clone)]
pub struct HostOptions {
    /// Directory for this host's database, credentials and instance homes.
    pub data_dir: PathBuf,
    /// Name shown to clients.
    pub name: String,
    /// Home remote, or `None` to use the stored remote.
    pub home: Option<String>,
    /// Directories scanned for project checkouts.
    pub projects: Vec<PathBuf>,
    /// The user's home, whose harness folders take the home's skills after
    /// each sync; `None` installs none.
    pub user: Option<PathBuf>,
}

/// A running host.
pub struct Hosted {
    router: Router,
    log: Log,
    token: proto::AuthToken,
    initialized: tokio::sync::watch::Receiver<bool>,
    fleet_lock: std::fs::File,
    has_fleet: bool,
}

impl Hosted {
    /// Address and authentication token for clients.
    #[must_use]
    pub fn target(&self) -> Target {
        Target {
            addr: self.router.endpoint().addr(),
            token: self.token,
        }
    }

    /// Waits for the stored instances to finish configuring while the host serves.
    ///
    /// # Errors
    ///
    /// The registry stopped before it published its initial offers.
    pub async fn initialized(&self) -> anyhow::Result<()> {
        self.initialized
            .clone()
            .wait_for(|ready| *ready)
            .await
            .context("the registry stopped during bootstrap")?;
        Ok(())
    }

    /// Stops accepting connections, closes the endpoint and closes the event log.
    ///
    /// # Errors
    ///
    /// The router or event log failed to shut down.
    pub async fn shutdown(self) -> anyhow::Result<()> {
        let Self {
            router,
            log,
            fleet_lock: _fleet_lock,
            ..
        } = self;
        let stopped = router.shutdown().await;
        drop(router);
        log.shutdown().await?;
        stopped?;
        Ok(())
    }
}

/// Starts serving on `endpoint` after session recovery. Stored instances may
/// still be probing. [`Hosted::initialized`] waits for their offers.
///
/// # Errors
///
/// The host's persistent state could not be opened or recovered.
pub async fn start(
    endpoint: Endpoint,
    plugins: Plugins,
    options: HostOptions,
) -> anyhow::Result<Hosted> {
    let (lock, fleet) = open_fleet(&endpoint, &options.data_dir).await?;
    let has_fleet = fleet.is_some();
    let data = options.data_dir.clone();
    let (fleet_slot, _) = tokio::sync::watch::channel(fleet);
    let started =
        assemble(plugins, options, fleet_slot.clone(), endpoint.clone()).await;
    match started {
        Ok((conductor, log, token, initialized)) => {
            let protocol = crate::fleet::Protocol::new(
                fleet_slot.clone(),
                data,
                endpoint.id(),
                token,
            );
            let router = Router::builder(endpoint.clone())
                .accept(proto::ALPN, conductor)
                .accept(proto::fleet::ALPN, protocol)
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
    fleet: crate::fleet::Slot,
    endpoint: Endpoint,
) -> anyhow::Result<(
    Conductor,
    Log,
    proto::AuthToken,
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
    });
    let (registry, initialized) = Registry::spawn(
        log.clone(),
        store,
        &stored,
        Providers::new(plugins.clone(), data_dir, fleet.clone()),
        home.clone(),
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

/// An instance's plugin, home and work dir, and its settings over the
/// provider's.
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

/// Installs the build of the plugin's CLI that it pins.
async fn install(plugin: &Plugin, data_dir: &Path) -> Result<PathBuf, String> {
    let pin = (plugin.pin)().map_err(|err| err.to_string())?;
    install::install(plugin.profile.provider.name(), pin, data_dir)
        .await
        .map_err(|err| err.to_string())
}

/// Runs each step of `host::start` for the instance's CLI: the one its
/// settings give, or else the pinned one installed into the data dir.
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

/// Signs an instance's CLI in on this terminal, into the instance's
/// private home, as the app's sign-in does on pipes.
///
/// The instance is `provider`'s of `account`, or without one the
/// provider's own. Once the CLI names the account it signed in as, the home is that
/// account's instance, as after a sign-in from the app; the host takes it
/// when it next starts.
///
/// # Errors
///
/// The host has no such instance, `plugins` has none for its provider,
/// the install or the sign-in failed, or the store couldn't keep the
/// account's instance.
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

pub fn data_dir_or_default(
    data_dir: Option<PathBuf>,
) -> anyhow::Result<PathBuf> {
    match data_dir {
        Some(dir) => Ok(dir),
        None => Ok(directories::ProjectDirs::from("", "", "conductor")
            .context("no home directory to put the data dir in")?
            .data_dir()
            .to_path_buf()),
    }
}

/// How long the sign-in check may take before the host counts the home
/// as signed out.
const SIGN_IN_CHECK: std::time::Duration = std::time::Duration::from_secs(15);

/// How the CLI's own sign-in check against its private home exited, and
/// what it printed, if it ran to the end within `SIGN_IN_CHECK`.
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

/// How the CLI's `--version` run exited.
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

/// Prints the ticket, the join link and its QR code, and writes the code as
/// an image to the data dir.
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
            // Terminals usually draw light glyphs on a dark background, so swap
            // the module colours; phone cameras often refuse inverted codes.
            writeln!(
                out,
                "{}",
                code.render::<Dense1x2>()
                    .dark_color(Dense1x2::Light)
                    .light_color(Dense1x2::Dark)
                    .quiet_zone(true)
                    .build()
            )?;
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

/// n0's public relays without the root dot. A host advertises its home relay
/// to peers, and a browser that dials the dotted name fails `WebKit`'s
/// certificate check (`net::without_root_dot`).
fn n0_relays() -> iroh::RelayMap {
    iroh::defaults::prod::default_relay_map()
        .urls::<Vec<RelayUrl>>()
        .iter()
        .map(net::without_root_dot)
        .collect()
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
