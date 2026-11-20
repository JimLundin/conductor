//! `conductord`'s commands, for the plugins it is handed: start the host,
//! or sign an instance's CLI in.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::credentials::{Store, Stored};
use crate::home::{HomeClone, Homing};
use crate::log::Log;
use crate::registry::{Launcher, Registry};
use crate::serve::Conductor;
use crate::sign_in::Signing;
use agents::runtime::Command;
use anyhow::Context;
use host::start::{Next, Output};
use install::process::command;
use install::{Home, install};
use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh::{Endpoint, RelayMode, RelayUrl};
use net::Target;
use proto::{Account, Catalog, Instance, InstanceId, Provider, Ticket};
use qrcode::render::svg;
use qrcode::render::unicode::Dense1x2;
use runtime::{Launch, Plugin, Settings};
use tracing::Instrument;

/// The providers a host runs, each by its plugin.
#[derive(Debug, Clone, Default)]
pub struct Plugins(Arc<BTreeMap<Provider, Plugin>>);

impl FromIterator<Plugin> for Plugins {
    fn from_iter<I: IntoIterator<Item = Plugin>>(plugins: I) -> Self {
        Self(Arc::new(
            plugins
                .into_iter()
                .map(|plugin| (plugin.profile.provider, plugin))
                .collect(),
        ))
    }
}

impl Plugins {
    /// How each provider presents itself, as its plugin says.
    #[must_use]
    fn catalog(&self) -> Catalog {
        Catalog(
            self.0
                .iter()
                .map(|(&provider, plugin)| {
                    (provider, plugin.profile.info.clone())
                })
                .collect(),
        )
    }
}

#[derive(Debug, Clone)]
pub struct Providers {
    plugins: Plugins,
    data_dir: PathBuf,
}

impl Providers {
    pub const fn new(plugins: Plugins, data_dir: PathBuf) -> Self {
        Self { plugins, data_dir }
    }

    pub fn catalog(&self) -> Catalog {
        self.plugins.catalog()
    }

    pub fn instructions_file(&self, provider: Provider) -> Option<String> {
        self.plugins
            .0
            .get(&provider)
            .map(|plugin| plugin.profile.instructions_file.clone())
    }

    pub async fn launch(
        &self,
        id: &InstanceId,
        instance: Instance,
    ) -> Launcher {
        let span = tracing::info_span!("instance", id = %id);
        match prepare(&self.plugins, &self.data_dir, id, &instance) {
            Ok(prepared) => {
                run(prepared, &self.data_dir, instance)
                    .instrument(span)
                    .await
            }
            Err(reason) => Launcher::Unavailable(reason),
        }
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
        let profile = plugin.profile;
        let program = match settings.path {
            Some(given) => given,
            None => install(profile.pin, &self.data_dir)
                .await
                .map_err(|err| err.to_string())?,
        };
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
        },
    )
    .await?;
    let target = hosted.target();
    let ticket = Ticket {
        host: *target.addr.id.as_bytes(),
        relay: relay.to_string(),
        token: target.token,
    };
    let ended = match announce(&data_dir, &web_url, &ticket) {
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
}

/// A running host.
pub struct Hosted {
    router: Router,
    log: Log,
    token: proto::AuthToken,
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

    /// Stops accepting connections, closes the endpoint and closes the event log.
    ///
    /// # Errors
    ///
    /// The router or event log failed to shut down.
    pub async fn shutdown(self) -> anyhow::Result<()> {
        let Self { router, log, .. } = self;
        let stopped = router.shutdown().await;
        drop(router);
        log.shutdown().await?;
        stopped?;
        Ok(())
    }
}

/// Starts the daemon on `endpoint`, using the given provider plugins.
///
/// # Errors
///
/// The host's persistent state could not be opened or recovered.
pub async fn start(
    endpoint: Endpoint,
    plugins: Plugins,
    options: HostOptions,
) -> anyhow::Result<Hosted> {
    let started = assemble(plugins, options).await;
    match started {
        Ok((conductor, log, token)) => {
            let router = Router::builder(endpoint)
                .accept(proto::ALPN, conductor)
                .spawn();
            Ok(Hosted { router, log, token })
        }
        Err(error) => {
            endpoint.close().await;
            Err(error)
        }
    }
}

async fn assemble(
    plugins: Plugins,
    options: HostOptions,
) -> anyhow::Result<(Conductor, Log, proto::AuthToken)> {
    let HostOptions {
        data_dir,
        name,
        home: remote,
        projects,
    } = options;
    let token = crate::load_or_create_token(&data_dir)?;
    let (store, stored) = Store::open(&data_dir)?;
    let log = Log::open(name.clone(), &data_dir.join("conductor.redb"))?;
    let home = HomeClone::spawn(Homing {
        host: name,
        data_dir: data_dir.clone(),
        remote,
        roots: projects,
    });
    let registry = Registry::spawn(
        log.clone(),
        store,
        &stored,
        Providers::new(plugins, data_dir),
        home.clone(),
    )
    .await?;
    let conductor = Conductor::new(log.clone(), registry, home, token);
    Ok((conductor, log, token))
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

/// Runs each step of `host::start` for the instance's CLI: the one its
/// settings give, or else the pinned one installed into the data dir.
async fn run(
    prepared: Prepared,
    data_dir: &Path,
    instance: Instance,
) -> Launcher {
    let Prepared {
        plugin,
        settings,
        home,
        work,
    } = prepared;
    let profile = plugin.profile;
    let mut next = host::start::start(
        profile.provider.name(),
        settings.path,
        settings.model,
    );
    loop {
        next = match next {
            Next::Install(installing) => installing.installed(
                install(profile.pin, data_dir)
                    .await
                    .map_err(|err| err.to_string()),
            ),
            Next::Probe(path, probing) => {
                let start = (plugin.starter)(Launch {
                    path: path.clone(),
                    home: home.clone(),
                    work: work.clone(),
                });
                let program = path.display().to_string();
                let ran =
                    version(command(&path, &home, profile.version.clone()))
                        .await;
                probing.probed(path, start, program, ran)
            }
            Next::CheckSignIn(path, checking) => checking.checked(
                &sign_in_check(command(&path, &home, profile.status.clone()))
                    .await
                    .map(|(code, _)| code),
            ),
            Next::ListModels(listing) => {
                let mut runtime = listing.start().start();
                let named = runtime
                    .ask(Command::Credential(instance.credential.clone()))
                    .await
                    .configured();
                let listed = runtime.ask(Command::Models).await.models();
                listing.listed(named, listed)
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
    let (store, stored) = Store::open(&data_dir)?;
    let roster = stored.roster(&plugins.catalog());
    let instance =
        host::registry::picked(&roster, provider, account.as_deref())
            .map_err(anyhow::Error::msg)?;
    let settings = roster
        .of(&instance)
        .with_context(|| format!("this host has no instance {instance}"))?;
    let providers = Providers::new(plugins, data_dir);
    let signing = providers
        .signing(&instance, settings)
        .await
        .map_err(anyhow::Error::msg)?;
    let status =
        command(&signing.program, &signing.home, signing.flow.run.clone())
            .status()
            .await?;
    host::start::signed_in(status.code(), &status.to_string())
        .map_err(anyhow::Error::msg)?;
    let check = command(&signing.program, &signing.home, signing.check.clone());
    let (_, printed) =
        sign_in_check(check).await.map_err(anyhow::Error::msg)?;
    let named = signing.account(&printed).map(Account::Named);
    let Some((next, to)) =
        host::registry::signed_in_as(&roster, &instance, named)
    else {
        return Ok(());
    };
    if let Stored::Unreadable(reason) = stored {
        anyhow::bail!(
            "signed in, but the store doesn't open to keep {to}: {reason}"
        );
    }
    move_instance(&providers.data_dir, &instance, &to)?;
    store.save(&next)?;
    tracing::info!(target: "host", instance = %to, "signed in");
    Ok(())
}

/// Moves `from`'s private home and work dir to `to`'s, in place of any
/// `to` had: a sign-in of `from` named `to`'s account.
///
/// # Errors
///
/// A directory couldn't be removed or moved.
pub fn move_instance(
    data_dir: &Path,
    from: &InstanceId,
    to: &InstanceId,
) -> anyhow::Result<()> {
    ["home", "work"].into_iter().try_for_each(|kind| {
        let source = data_dir.join(kind).join(host::registry::dir(from));
        let target = data_dir.join(kind).join(host::registry::dir(to));
        clear(&target).and_then(|()| rename_existing(&source, &target))
    })
}

/// Removes the directory at `target`, if there is one.
fn clear(target: &Path) -> anyhow::Result<()> {
    if target.exists() {
        std::fs::remove_dir_all(target)
            .with_context(|| format!("removing {}", target.display()))?;
    }
    Ok(())
}

/// Moves the directory at `source`, if there is one, to `target`.
fn rename_existing(source: &Path, target: &Path) -> anyhow::Result<()> {
    if source.exists() {
        std::fs::rename(source, target).with_context(|| {
            format!("moving {} to {}", source.display(), target.display())
        })?;
    }
    Ok(())
}

fn private_home(
    data_dir: &Path,
    id: &InstanceId,
    plugin: &Plugin,
) -> Result<Home, String> {
    Home::private(data_dir, &host::registry::dir(id), &plugin.profile.config)
        .map_err(|err| format!("creating the private home for {id}: {err}"))
}

fn data_dir_or_default(data_dir: Option<PathBuf>) -> anyhow::Result<PathBuf> {
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
async fn version(mut cli: tokio::process::Command) -> Result<Output, String> {
    cli.output()
        .await
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
            match std::fs::write(&svg_path, image) {
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
