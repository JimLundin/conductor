use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agents::codex::ServerOptions;
use anyhow::Context;
use clap::{Parser, Subcommand};
use daemon::codex_hub::CodexHub;
use daemon::log::Log;
use daemon::registry::{Launcher, Launchers, Registry};
use daemon::rtc::{Blackhole, RtcOptions};
use daemon::serve::Conductor;
use daemon::session::{ClaudeLaunch, CodexLaunch, Launch};
use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh::{Endpoint, RelayMode, RelayUrl};
use iroh_mdns_address_lookup::MdnsAddressLookup;
use proto::Ticket;
use qrcode::render::svg;
use qrcode::render::unicode::Dense1x2;

#[derive(Parser)]
#[command(name = "conductord", about = "The Conductor host daemon")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Serve this host's sessions over iroh and print a join ticket.
    Host(HostArgs),
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
    /// A STUN server for the WebRTC upgrade, as `stun:host:port`. Repeat
    /// for more. STUN only reports public addresses; nothing relays.
    #[arg(long = "stun", default_values = proto::DEFAULT_STUN_SERVERS)]
    stun_servers: Vec<String>,
    /// Refuse browsers' WebRTC upgrades, so they stay on iroh's relay.
    #[arg(long = "no-webrtc", action = clap::ArgAction::SetFalse)]
    webrtc: bool,
    /// Test only: drop every WebRTC datagram, so upgrades fail.
    #[arg(long, hide = true)]
    test_rtc_blackhole: bool,
    /// Test only: drop the first WebRTC connection after it sends this
    /// many frames.
    #[arg(long, hide = true)]
    test_rtc_drop_after: Option<u64>,
    /// The Claude Code CLI. A host where it doesn't run offers no Claude
    /// sessions.
    #[arg(long, default_value = "claude")]
    claude_path: PathBuf,
    /// The model for Claude sessions, such as `haiku`. Defaults to the CLI's.
    #[arg(long)]
    claude_model: Option<String>,
    /// The Codex CLI. A host where it doesn't run offers no Codex sessions.
    #[arg(long, default_value = "codex")]
    codex_path: PathBuf,
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
    match Cli::parse().command {
        Cmd::Host(HostArgs {
            data_dir,
            name,
            web_url,
            stun_servers,
            webrtc,
            test_rtc_blackhole,
            test_rtc_drop_after,
            claude_path,
            claude_model,
            codex_path,
            codex_model,
        }) => {
            let data_dir = match data_dir {
                Some(dir) => dir,
                None => directories::ProjectDirs::from("", "", "conductor")
                    .context("no home directory to put the data dir in")?
                    .data_dir()
                    .to_path_buf(),
            };
            let rtc = webrtc.then_some(Arc::new(RtcOptions::new(
                stun_servers,
                test_rtc_blackhole.then_some(Blackhole {}),
                test_rtc_drop_after,
            )));
            let claude = launcher(
                &claude_path.clone(),
                Launch::Claude(ClaudeLaunch {
                    claude_path,
                    model: claude_model,
                }),
            )
            .await;
            // The app-server's own working directory is the data dir; each
            // thread runs in its session's directory.
            let hub = CodexHub::spawn(ServerOptions {
                codex_path: codex_path.clone(),
                cwd: data_dir.clone(),
                stdout_log: None,
            });
            let codex = launcher(
                &codex_path,
                Launch::Codex(CodexLaunch {
                    hub,
                    model: codex_model,
                }),
            )
            .await;
            let launchers = Launchers { claude, codex };
            let router = host(data_dir, web_url, name, rtc, launchers).await?;
            tokio::signal::ctrl_c().await?;
            router.shutdown().await?;
            Ok(())
        }
    }
}

async fn host(
    data_dir: PathBuf,
    web_url: String,
    name: String,
    rtc: Option<Arc<RtcOptions>>,
    launchers: Launchers,
) -> anyhow::Result<Router> {
    let endpoint = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Custom(n0_relays()))
        .secret_key(daemon::load_or_create_key(&data_dir)?)
        .address_lookup(MdnsAddressLookup::builder())
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
    let log = Log::spawn(name, daemon::random_epoch(), launchers.providers());
    let conductor = Conductor {
        log: log.clone(),
        registry: Registry::spawn(log, launchers),
        token,
        rtc,
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

/// A provider is available when `<path> --version` runs and succeeds.
async fn launcher(cli: &Path, launch: Launch) -> Launcher {
    let ran = tokio::process::Command::new(cli)
        .arg("--version")
        .output()
        .await;
    let path = cli.display().to_string();
    match ran {
        Ok(output) => match output.status.code() {
            Some(0) => {
                let version = String::from_utf8_lossy(&output.stdout);
                tracing::info!(%path, version = %version.trim(), "available");
                Launcher::Ready(launch)
            }
            status => Launcher::Unavailable(format!("{path} --version exited with {status:?}")),
        },
        Err(err) => Launcher::Unavailable(format!("{path} does not run: {err}")),
    }
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
