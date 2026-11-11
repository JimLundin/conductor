use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use clap::{Parser, Subcommand};
use daemon::log::Log;
use daemon::registry::{Availability, Providers, Registry};
use daemon::rtc::{Blackhole, RtcOptions};
use daemon::serve::Conductor;
use daemon::session::{ClaudeLaunch, Launch};
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
}

/// Where the host is reachable, as the ticket tells clients.
struct Serving {
    data_dir: PathBuf,
    web_url: String,
    router: Router,
    ticket: Ticket,
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
            let claude = claude(ClaudeLaunch {
                claude_path,
                model: claude_model,
            })
            .await;
            let serving = host(data_dir, web_url, name, rtc, claude).await?;
            announce(&serving)?;
            tokio::signal::ctrl_c().await?;
            serving.router.shutdown().await?;
            Ok(())
        }
    }
}

async fn host(
    data_dir: PathBuf,
    web_url: String,
    name: String,
    rtc: Option<Arc<RtcOptions>>,
    claude: Availability,
) -> anyhow::Result<Serving> {
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
    let log = Log::spawn(name, daemon::random_epoch());
    let codex =
        Availability::Unavailable(String::from("this host does not run Codex sessions yet"));
    let conductor = Conductor {
        log: log.clone(),
        registry: Registry::spawn(log, Providers { claude, codex }),
        token,
        rtc,
    };
    Ok(Serving {
        data_dir,
        web_url,
        ticket: Ticket {
            host: endpoint.id(),
            relay,
            token,
        },
        router: Router::builder(endpoint)
            .accept(proto::ALPN, conductor)
            .spawn(),
    })
}

/// Claude is available when `claude --version` runs and succeeds.
async fn claude(launch: ClaudeLaunch) -> Availability {
    let ran = tokio::process::Command::new(&launch.claude_path)
        .arg("--version")
        .output()
        .await;
    let path = launch.claude_path.display().to_string();
    match ran {
        Ok(output) => match output.status.code() {
            Some(0) => {
                let version = String::from_utf8_lossy(&output.stdout);
                tracing::info!(%path, version = %version.trim(), "claude is available");
                Availability::Ready(Launch::Claude(launch))
            }
            status => Availability::Unavailable(format!("{path} --version exited with {status:?}")),
        },
        Err(err) => Availability::Unavailable(format!("{path} does not run: {err}")),
    }
}

/// Prints the ticket, the join link and its QR code, and writes the code as
/// an image to the data dir.
fn announce(serving: &Serving) -> anyhow::Result<()> {
    let ticket = serving.ticket.to_text();
    let join = format!("{}#join={ticket}", serving.web_url);
    let mut out = std::io::stdout();
    writeln!(out, "data dir: {}", serving.data_dir.display())?;
    writeln!(out, "host id:  {}", serving.ticket.host)?;
    writeln!(out, "relay:    {}", serving.ticket.relay)?;
    writeln!(out, "ticket:   {ticket}")?;
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
            let svg_path = serving.data_dir.join("join-qr.svg");
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
