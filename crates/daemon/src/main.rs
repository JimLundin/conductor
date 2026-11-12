use std::io::Write;
use std::path::{Path, PathBuf};

use agents::codex::ServerOptions;
use agents::install::{Cli, install};
use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use daemon::codex_hub::CodexHub;
use daemon::credentials::{Store, Stored};
use daemon::log::Log;
use daemon::registry::{Launcher, Launchers, OwnSignIn, Registry};
use daemon::serve::Conductor;
use daemon::session::{ClaudeLaunch, CodexLaunch, Launch, claude_models};
use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh::{Endpoint, RelayMode, RelayUrl};
use proto::{ModelId, Models, Ticket};
use qrcode::render::svg;
use qrcode::render::unicode::Dense1x2;

#[derive(Parser)]
#[command(name = "conductord", about = "The Conductor host daemon")]
struct Args {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Serve this host's sessions over iroh and print a join ticket.
    Host(HostArgs),
    /// Sign a provider's CLI in on this terminal, into the host's private
    /// home for it. Until the app can sign in (docs/roadmap.md).
    Login(LoginArgs),
}

#[derive(Parser)]
struct LoginArgs {
    provider: LoginProvider,
    /// The host's data dir, as given to `conductord host`.
    #[arg(long)]
    data_dir: Option<PathBuf>,
}

#[derive(Clone, Copy, ValueEnum)]
enum LoginProvider {
    /// `claude auth login --claudeai`: a Claude subscription.
    Claude,
    /// `codex login --device-auth`: ChatGPT, by device code.
    Codex,
}

#[derive(Parser)]
struct HostArgs {
    /// Where the host key lives. Defaults to the OS data dir for "conductor".
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// The name clients show for this host.
    #[arg(long, default_value = "conductor")]
    name: String,
    /// The web app's URL; the printed join link is this plus `#join=<ticket>`.
    #[arg(long, default_value = "http://localhost:8080/conductor/")]
    web_url: String,
    /// A Claude Code CLI to run instead of the pinned one the host installs,
    /// for tests and development. A host where it doesn't run offers no
    /// Claude sessions.
    #[arg(long)]
    claude_path: Option<PathBuf>,
    /// The model for Claude sessions, such as `haiku`. Defaults to the CLI's.
    #[arg(long)]
    claude_model: Option<String>,
    /// A Codex CLI to run instead of the pinned one the host installs, for
    /// tests and development. A host where it doesn't run offers no Codex
    /// sessions.
    #[arg(long)]
    codex_path: Option<PathBuf>,
    /// The model for Codex sessions, such as `gpt-6-luna`. Defaults to the
    /// app-server's.
    #[arg(long)]
    codex_model: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,iroh=warn,noq=warn,swarm_discovery=warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    match Args::parse().command {
        Cmd::Host(HostArgs {
            data_dir,
            name,
            web_url,
            claude_path,
            claude_model,
            codex_path,
            codex_model,
        }) => {
            let data_dir = data_dir_or_default(data_dir)?;
            let homes = daemon::homes(&data_dir)?;
            let (store, stored) = Store::open(&data_dir)?;
            let claude = match cli_path(Cli::Claude, claude_path, &data_dir).await {
                Ok((claude_path, origin)) => {
                    let status = agents::claude::auth_status(&claude_path, &homes.claude);
                    let launch = ClaudeLaunch {
                        claude_path: claude_path.clone(),
                        home: homes.claude.clone(),
                        credential: None,
                    };
                    // The CLI lists its models as a session would see them,
                    // started in a directory of its own.
                    let probe = data_dir.join("probe");
                    std::fs::create_dir_all(&probe)?;
                    let listed = claude_models(
                        ClaudeLaunch {
                            credential: match &stored {
                                Stored::Opened(credentials) => credentials.claude.clone(),
                                Stored::Unreadable(_) => None,
                            },
                            ..launch.clone()
                        },
                        probe,
                    );
                    match runs(agents::claude::cli(&claude_path, &homes.claude)).await {
                        Ok(cli) => ready(
                            cli,
                            Launch::Claude(launch),
                            own_sign_in(origin, status).await,
                            (listed.await, claude_model),
                        ),
                        Err(reason) => unavailable(reason),
                    }
                }
                Err(reason) => unavailable(reason),
            };
            let codex = match cli_path(Cli::Codex, codex_path, &data_dir).await {
                Ok((codex_path, origin)) => {
                    let version = agents::codex::cli(&codex_path, &homes.codex);
                    let status = agents::codex::login_status(&codex_path, &homes.codex);
                    // The app-server's own working directory is the data dir;
                    // each thread runs in its session's directory.
                    let hub = CodexHub::spawn(ServerOptions {
                        codex_path,
                        home: homes.codex,
                        credential: match &stored {
                            Stored::Opened(credentials) => credentials.codex.clone(),
                            Stored::Unreadable(_) => None,
                        },
                        cwd: data_dir.clone(),
                        stdout_log: None,
                    });
                    let lister = hub.clone();
                    let listed = lister.models();
                    match runs(version).await {
                        Ok(cli) => ready(
                            cli,
                            Launch::Codex(CodexLaunch { hub }),
                            own_sign_in(origin, status).await,
                            (listed.await.map_err(|err| err.to_string()), codex_model),
                        ),
                        Err(reason) => unavailable(reason),
                    }
                }
                Err(reason) => unavailable(reason),
            };
            let launchers = Launchers { claude, codex };
            let router = host(data_dir, web_url, name, launchers, store, stored).await?;
            tokio::signal::ctrl_c().await?;
            router.shutdown().await?;
            Ok(())
        }
        Cmd::Login(LoginArgs { provider, data_dir }) => {
            let data_dir = data_dir_or_default(data_dir)?;
            let homes = daemon::homes(&data_dir)?;
            let mut login = match provider {
                LoginProvider::Claude => {
                    agents::claude::login(&install(Cli::Claude, &data_dir).await?, &homes.claude)
                }
                LoginProvider::Codex => {
                    agents::codex::login(&install(Cli::Codex, &data_dir).await?, &homes.codex)
                }
            };
            let status = login.status().await?;
            match status.code() {
                Some(0) => Ok(()),
                _ => Err(anyhow::Error::msg(format!(
                    "the sign-in exited with {status}"
                ))),
            }
        }
    }
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

/// Where the CLI the host runs came from.
enum Origin {
    Given,
    Pinned,
}

/// The CLI to run: the one given, or else the pinned one, installed into the
/// data dir if it isn't there yet.
async fn cli_path(
    cli: Cli,
    given: Option<PathBuf>,
    data_dir: &Path,
) -> Result<(PathBuf, Origin), String> {
    match given {
        Some(path) => Ok((path, Origin::Given)),
        None => install(cli, data_dir)
            .await
            .map(|path| (path, Origin::Pinned))
            .map_err(|err| format!("installing the pinned {}: {err}", cli.name())),
    }
}

/// How long the sign-in check may take before the host counts the home
/// as signed out.
const SIGN_IN_CHECK: std::time::Duration = std::time::Duration::from_secs(15);

/// A given CLI signs in however it does. A pinned one is signed in when
/// `status`, its own sign-in check against the private home, succeeds
/// within `SIGN_IN_CHECK`. It runs once, at start.
async fn own_sign_in(origin: Origin, mut status: tokio::process::Command) -> OwnSignIn {
    status.kill_on_drop(true);
    match origin {
        Origin::Given => OwnSignIn::Unmanaged,
        Origin::Pinned => match tokio::time::timeout(SIGN_IN_CHECK, status.output())
            .await
            .map(|output| output.map(|output| output.status.code()))
        {
            Ok(Ok(Some(0))) => OwnSignIn::SignedIn,
            Ok(Ok(_) | Err(_)) | Err(_) => OwnSignIn::Missing,
        },
    }
}

async fn host(
    data_dir: PathBuf,
    web_url: String,
    name: String,
    launchers: Launchers,
    store: Store,
    stored: Stored,
) -> anyhow::Result<Router> {
    let endpoint = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Custom(n0_relays()))
        .secret_key(daemon::load_or_create_key(&data_dir)?)
        .bind()
        .await?;
    endpoint.online().await;
    let relay = endpoint
        .addr()
        .relay_urls()
        .next()
        .cloned()
        .context("online, but no home relay")?;
    let token = daemon::random_token();
    let log = Log::spawn(name, daemon::random_epoch());
    let conductor = Conductor {
        log: log.clone(),
        registry: Registry::spawn(log, launchers, store, stored),
        token,
    };
    let ticket = Ticket {
        host: endpoint.id(),
        relay,
        token,
    };
    let router = Router::builder(endpoint)
        .accept(proto::ALPN, conductor)
        .spawn();
    announce(&data_dir, &web_url, &ticket)?;
    Ok(router)
}

/// The CLI's path and version, when `<cli> --version` runs and succeeds.
async fn runs(mut cli: tokio::process::Command) -> Result<(String, String), String> {
    let path = cli.as_std().get_program().display().to_string();
    match cli.arg("--version").output().await {
        Ok(output) => match output.status.code() {
            Some(0) => Ok((
                path,
                String::from(String::from_utf8_lossy(&output.stdout).trim()),
            )),
            status => Err(format!("{path} --version exited with {status:?}")),
        },
        Err(err) => Err(format!("{path} does not run: {err}")),
    }
}

/// A CLI that runs offers the models it `listed`, with `default`
/// (`--claude-model`, `--codex-model`) first as the one a session gets when
/// it names none. A CLI that could not list them, for instance because it
/// is not signed in yet, offers only the default.
fn ready(
    cli: (String, String),
    launch: Launch,
    own: OwnSignIn,
    models: (Result<Models, String>, Option<String>),
) -> Launcher {
    let (listed, default) = models;
    let listed = listed.unwrap_or_else(|reason| {
        tracing::warn!(%reason, "the CLI listed no models");
        Models::none()
    });
    let models = Models {
        default: default.map(ModelId).or(listed.default),
        ..listed
    };
    let (path, version) = cli;
    tracing::info!(%path, %version, ?own, models = models.choices.len(), "ready");
    Launcher::Ready {
        launch,
        own,
        models,
    }
}

fn unavailable(reason: String) -> Launcher {
    tracing::warn!("unavailable: {reason}");
    Launcher::Unavailable(reason)
}

/// Prints the ticket, the join link and its QR code, and writes the code as
/// an image to the data dir.
fn announce(data_dir: &Path, web_url: &str, ticket: &Ticket) -> anyhow::Result<()> {
    let text = ticket.to_text();
    let join = format!("{web_url}#join={text}");
    let mut out = std::io::stdout();
    writeln!(out, "data dir: {}", data_dir.display())?;
    writeln!(out, "host id:  {}", ticket.host)?;
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
            let image = code.render::<svg::Color>().min_dimensions(512, 512).build();
            match std::fs::write(&svg_path, image) {
                Ok(()) => writeln!(out, "qr image: {}", svg_path.display())?,
                Err(err) => tracing::warn!("could not write {}: {err}", svg_path.display()),
            }
        }
        Err(err) => tracing::warn!("could not render the join link as a QR code: {err}"),
    }
    Ok(())
}

/// n0's public relays without the root dot. A host advertises its home relay
/// to peers, and a browser that dials the dotted name fails WebKit's
/// certificate check (`proto::without_root_dot`).
fn n0_relays() -> iroh::RelayMap {
    iroh::defaults::prod::default_relay_map()
        .urls::<Vec<RelayUrl>>()
        .iter()
        .map(proto::without_root_dot)
        .collect()
}
