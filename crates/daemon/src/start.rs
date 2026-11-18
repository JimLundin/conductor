//! `conductord`'s commands, for the plugins it is handed: start the host,
//! or sign a provider's CLI in.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::credentials::{Store, Stored};
use crate::log::Log;
use crate::registry::{Launcher, Launchers, Registry};
use crate::serve::Conductor;
use agents::Profile;
use agents::runtime::Command;
use anyhow::Context;
use host::start::{Next, Output};
use install::process::command;
use install::{Home, install};
use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh::{Endpoint, RelayMode, RelayUrl};
use proto::{Catalog, Credentials, Provider, Ticket};
use qrcode::render::svg;
use qrcode::render::unicode::Dense1x2;
use runtime::{Launch, Plugin};

/// The providers a host runs, each by its plugin.
#[derive(Debug, Clone, Default)]
pub struct Plugins(BTreeMap<Provider, Plugin>);

impl FromIterator<Plugin> for Plugins {
    fn from_iter<I: IntoIterator<Item = Plugin>>(plugins: I) -> Self {
        Self(
            plugins
                .into_iter()
                .map(|plugin| (plugin.profile.provider, plugin))
                .collect(),
        )
    }
}

impl Plugins {
    /// How each provider presents itself, as its plugin says.
    #[must_use]
    pub fn catalog(&self) -> Catalog {
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

/// What the registry starts with.
struct Registered {
    launchers: Launchers,
    store: Store,
    stored: Stored,
    catalog: Catalog,
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
    } = hosting;
    let data_dir = data_dir_or_default(given)?;
    let (store, stored) = Store::open(&data_dir)?;
    let credentials = stored.credentials();
    let catalog = plugins.catalog();
    let mut launchers = Launchers::new();
    for (provider, plugin) in plugins.0 {
        let launcher = launcher(plugin, &data_dir, &credentials).await?;
        launchers.insert(provider, launcher);
    }
    let registered = Registered {
        launchers,
        store,
        stored,
        catalog,
    };
    let Hosted { router, log } =
        listen(data_dir, web_url, name, registered).await?;
    let ended = tokio::select! {
        result = tokio::signal::ctrl_c() => result.map_err(anyhow::Error::from),
        () = log.stopped() => Err(anyhow::Error::from(crate::Stopped {})),
    };
    router.shutdown().await?;
    drop(router);
    ended?;
    Ok(())
}

/// Runs each step of `host::start` for `plugin`'s CLI: the one its
/// settings give, or else the pinned one installed into the data dir.
async fn launcher(
    plugin: Plugin,
    data_dir: &Path,
    credentials: &Credentials,
) -> anyhow::Result<Launcher> {
    let Plugin {
        profile,
        settings,
        starter,
    } = plugin;
    let home = private_home(data_dir, &profile)?;
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
                let start = starter(Launch {
                    path: path.clone(),
                    home: home.clone(),
                    data_dir: data_dir.to_path_buf(),
                });
                let program = path.display().to_string();
                let ran =
                    version(command(&path, &home, profile.version.clone()))
                        .await;
                probing.probed(path, start, program, ran)
            }
            Next::CheckSignIn(path, checking) => checking.checked(
                &sign_in_check(command(&path, &home, profile.status.clone()))
                    .await,
            ),
            Next::ListModels(listing) => {
                let mut runtime = listing.start().start();
                let named = runtime
                    .ask(Command::Credentials(credentials.clone()))
                    .await
                    .configured();
                let listed = runtime.ask(Command::Models).await.models();
                listing.listed(named, listed)
            }
            Next::Done(launcher, notes) => {
                notes.iter().for_each(crate::log_note);
                break Ok(launcher);
            }
        }
    }
}

/// Signs `provider`'s pinned CLI in on this terminal, into the host's
/// private home for it. Until the app can sign in (docs/roadmap.md).
///
/// # Errors
///
/// `plugins` has none for `provider`, or the install or the sign-in failed.
pub async fn login(
    given: Option<PathBuf>,
    plugins: Plugins,
    provider: Provider,
) -> anyhow::Result<()> {
    let data_dir = data_dir_or_default(given)?;
    let profile = &plugins
        .0
        .get(&provider)
        .with_context(|| format!("no plugin runs {}", provider.name()))?
        .profile;
    let home = private_home(&data_dir, profile)?;
    let cli = install(profile.pin, &data_dir).await?;
    let status = command(&cli, &home, profile.login.clone()).status().await?;
    host::start::signed_in(status.code(), &status.to_string())
        .map_err(anyhow::Error::msg)
}

fn private_home(data_dir: &Path, profile: &Profile) -> anyhow::Result<Home> {
    Home::private(data_dir, profile).with_context(|| {
        format!("creating the private home for {}", profile.provider.name())
    })
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

/// How the CLI's own sign-in check against its private home exited, if it
/// ran to the end within `SIGN_IN_CHECK`.
async fn sign_in_check(
    mut status: tokio::process::Command,
) -> Result<Option<i32>, String> {
    status.kill_on_drop(true);
    tokio::time::timeout(SIGN_IN_CHECK, status.output())
        .await
        .map_err(|elapsed| elapsed.to_string())?
        .map(|output| output.status.code())
        .map_err(|err| err.to_string())
}

struct Hosted {
    router: Router,
    log: Log,
}

async fn listen(
    data_dir: PathBuf,
    web_url: String,
    name: String,
    registered: Registered,
) -> anyhow::Result<Hosted> {
    let Registered {
        launchers,
        store,
        stored,
        catalog,
    } = registered;
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
    let token = crate::load_or_create_token(&data_dir)?;
    let log = Log::open(name, &data_dir.join("conductor.redb"))?;
    let conductor = Conductor {
        log: log.clone(),
        registry: Registry::spawn(
            log.clone(),
            launchers,
            store,
            stored,
            catalog,
        ),
        token,
    };
    let ticket = Ticket {
        host: *endpoint.id().as_bytes(),
        relay: relay.to_string(),
        token,
    };
    let router = Router::builder(endpoint)
        .accept(proto::ALPN, conductor)
        .spawn();
    announce(&data_dir, &web_url, &ticket)?;
    Ok(Hosted { router, log })
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
