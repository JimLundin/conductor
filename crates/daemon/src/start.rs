//! `conductord`'s commands: start the host, or sign a CLI in.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::credentials::{Store, Stored};
use crate::log::Log;
use crate::registry::{Launcher, Launchers, Registry};
use crate::serve::Conductor;
use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use claude_cli::ClaudeLaunch;
use codex_cli::{Hub, ServerOptions};
use host::start::{Defaults, Next, Output};
use install::Home;
use install::{Cli, install};
use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh::{Endpoint, RelayMode, RelayUrl};
use proto::{Provider, Ticket};
use qrcode::render::svg;
use qrcode::render::unicode::Dense1x2;
use runtime::Starter;

#[derive(Parser)]
#[command(name = "conductord", about = "The Conductor host daemon")]
pub struct Args {
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
    /// `codex login --device-auth`: `ChatGPT`, by device code.
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

/// Runs the `conductord` command `args` names.
///
/// # Errors
///
/// Why the host could not start or stopped, or why the sign-in failed.
pub async fn run(args: Args) -> anyhow::Result<()> {
    match args.command {
        Cmd::Host(host_args) => serve_host(host_args).await,
        Cmd::Login(login_args) => login(login_args).await,
    }
}

async fn serve_host(args: HostArgs) -> anyhow::Result<()> {
    let HostArgs {
        data_dir: given_dir,
        name,
        web_url,
        claude_path,
        claude_model,
        codex_path,
        codex_model,
    } = args;
    let data_dir = data_dir_or_default(given_dir)?;
    let homes = crate::homes(&data_dir)?;
    let (store, stored) = Store::open(&data_dir)?;
    let claude = launcher(
        claude_kit(claude_model, &data_dir, homes.claude, &stored),
        claude_path,
        &data_dir,
    )
    .await;
    let codex = launcher(
        codex_kit(codex_model, &data_dir, homes.codex, &stored),
        codex_path,
        &data_dir,
    )
    .await;
    let launchers =
        Launchers::from([(Provider::Claude, claude), (Provider::Codex, codex)]);
    let Hosted { router, log } =
        host(data_dir, web_url, name, launchers, store, stored).await?;
    let ended = tokio::select! {
        result = tokio::signal::ctrl_c() => result.map_err(anyhow::Error::from),
        () = log.stopped() => Err(anyhow::Error::from(crate::Stopped {})),
    };
    router.shutdown().await?;
    drop(router);
    ended?;
    Ok(())
}

/// What starting one provider's CLI takes: its install, its probes and its
/// starter.
struct Kit {
    cli: Cli,
    home: Home,
    version: fn(&Path, &Home) -> tokio::process::Command,
    status: fn(&Path, &Home) -> tokio::process::Command,
    starter: Box<dyn Fn(&Path) -> Starter + Send + Sync>,
    defaults: Defaults,
}

fn claude_kit(
    model: Option<String>,
    data_dir: &Path,
    home: Home,
    stored: &Stored,
) -> Kit {
    let credential = claude::runtime::credential(&stored.credentials());
    let probe = data_dir.join("probe");
    let launched = home.clone();
    Kit {
        cli: Cli::Claude,
        home,
        version: claude_cli::cli,
        status: claude_cli::auth_status,
        starter: Box::new(move |path| {
            claude_cli::starter(ClaudeLaunch {
                claude_path: path.to_path_buf(),
                home: launched.clone(),
                credential: credential.clone(),
                probe: probe.clone(),
            })
        }),
        defaults: Defaults {
            named: None,
            chosen: model,
        },
    }
}

fn codex_kit(
    model: Option<String>,
    data_dir: &Path,
    home: Home,
    stored: &Stored,
) -> Kit {
    let credential = codex::runtime::credential(&stored.credentials());
    let named = credential.as_ref().map(codex::runtime::default_model);
    let cwd = data_dir.to_path_buf();
    let launched = home.clone();
    Kit {
        cli: Cli::Codex,
        home,
        version: codex_cli::cli,
        status: codex_cli::login_status,
        // The app-server's own working directory is the data dir;
        // each thread runs in its session's directory.
        starter: Box::new(move |path| {
            codex_cli::starter(Hub::spawn(ServerOptions {
                codex_path: path.to_path_buf(),
                home: launched.clone(),
                credential: credential.clone(),
                cwd: cwd.clone(),
                stdout_log: None,
            }))
        }),
        defaults: Defaults {
            named,
            chosen: model,
        },
    }
}

/// Runs each step of `host::start` for the CLI `kit` starts, the `given`
/// one or else the pinned one installed into the data dir.
async fn launcher(
    kit: Kit,
    given: Option<PathBuf>,
    data_dir: &Path,
) -> Launcher {
    let mut next = host::start::start(kit.cli.name(), given, kit.defaults);
    loop {
        next = match next {
            Next::Install(installing) => installing.installed(
                install(kit.cli, data_dir)
                    .await
                    .map_err(|err| err.to_string()),
            ),
            Next::Probe(path, probing) => {
                let start = (kit.starter)(&path);
                let program = path.display().to_string();
                let ran = version((kit.version)(&path, &kit.home)).await;
                probing.probed(path, start, program, ran)
            }
            Next::CheckSignIn(path, checking) => checking
                .checked(&sign_in_check((kit.status)(&path, &kit.home)).await),
            Next::ListModels(listing) => {
                let listed = listing.start().models().await;
                listing.listed(listed)
            }
            Next::Done(launcher, notes) => {
                notes.iter().for_each(crate::log_note);
                break launcher;
            }
        }
    }
}

async fn login(args: LoginArgs) -> anyhow::Result<()> {
    let data_dir = data_dir_or_default(args.data_dir)?;
    let homes = crate::homes(&data_dir)?;
    let mut sign_in = match args.provider {
        LoginProvider::Claude => claude_cli::login(
            &install(Cli::Claude, &data_dir).await?,
            &homes.claude,
        ),
        LoginProvider::Codex => codex_cli::login(
            &install(Cli::Codex, &data_dir).await?,
            &homes.codex,
        ),
    };
    let status = sign_in.status().await?;
    host::start::signed_in(status.code(), &status.to_string())
        .map_err(anyhow::Error::msg)
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

async fn host(
    data_dir: PathBuf,
    web_url: String,
    name: String,
    launchers: Launchers,
    store: Store,
    stored: Stored,
) -> anyhow::Result<Hosted> {
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
        registry: Registry::spawn(log.clone(), launchers, store, stored),
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

/// How `<cli> --version` exited.
async fn version(mut cli: tokio::process::Command) -> Result<Output, String> {
    cli.arg("--version")
        .output()
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
