use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use daemon::host::{Host, HostOptions};
use daemon::rtc::{Blackhole, RtcOptions};
use daemon::server::Conductor;
use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh::{Endpoint, RelayMode};
use iroh_mdns_address_lookup::MdnsAddressLookup;
use proto::Ticket;

#[derive(Parser)]
#[command(name = "conductord", about = "The Conductor host daemon")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Spike S3: serve a simulated agent over iroh and print a join ticket.
    S3Host {
        /// Where the host key lives. Defaults to the OS data dir for "conductor".
        #[arg(long)]
        data_dir: Option<PathBuf>,
        /// The name clients show for this host.
        #[arg(long, default_value = "s3-host")]
        name: String,
        /// The web app's URL; the printed join link is this plus `#join=<ticket>`.
        #[arg(long, default_value = "http://localhost:8080/conductor/")]
        web_url: String,
        /// Pause between the simulated agent's transcript chunks.
        #[arg(long, default_value_t = 100)]
        delta_ms: u64,
        /// A STUN server for the WebRTC upgrade, as `stun:host:port`. Repeat
        /// for more. STUN only reports public addresses; nothing relays.
        #[arg(long = "stun", default_values = proto::DEFAULT_STUN_SERVERS)]
        stun_servers: Vec<String>,
        /// Refuse browsers' WebRTC upgrades, so they stay on iroh's relay.
        #[arg(long)]
        no_webrtc: bool,
        /// Test only: drop every WebRTC datagram, so upgrades fail.
        #[arg(long, hide = true)]
        test_rtc_blackhole: bool,
        /// Test only: drop the first WebRTC connection after it sends this
        /// many frames.
        #[arg(long, hide = true)]
        test_rtc_drop_after: Option<u64>,
    },
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
        Cmd::S3Host {
            data_dir,
            name,
            web_url,
            delta_ms,
            stun_servers,
            no_webrtc,
            test_rtc_blackhole,
            test_rtc_drop_after,
        } => {
            let data_dir = match data_dir {
                Some(dir) => dir,
                None => directories::ProjectDirs::from("", "", "conductor")
                    .context("no home directory to put the data dir in")?
                    .data_dir()
                    .to_path_buf(),
            };
            let rtc = (!no_webrtc).then(|| {
                Arc::new(RtcOptions::new(
                    stun_servers,
                    test_rtc_blackhole.then_some(Blackhole {}),
                    test_rtc_drop_after,
                ))
            });
            s3_host(
                data_dir,
                name,
                web_url,
                Duration::from_millis(delta_ms),
                rtc,
            )
            .await
        }
    }
}

async fn s3_host(
    data_dir: PathBuf,
    name: String,
    web_url: String,
    delta_interval: Duration,
    rtc: Option<Arc<RtcOptions>>,
) -> anyhow::Result<()> {
    let key = daemon::load_or_create_key(&data_dir)?;
    let endpoint = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Custom(n0_relays()))
        .secret_key(key)
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
    let host = Host::spawn(HostOptions {
        name,
        epoch: daemon::random_epoch(),
        delta_interval,
    });
    let router = Router::builder(endpoint.clone())
        .accept(proto::ALPN, Conductor { host, token, rtc })
        .spawn();

    let ticket = Ticket {
        host: endpoint.id(),
        relay,
        token,
    };
    println!("data dir: {}", data_dir.display());
    println!("host id:  {}", endpoint.id());
    println!("relay:    {}", ticket.relay);
    let ticket = ticket.to_text();
    println!("ticket:   {ticket}");
    let join = format!("{web_url}#join={ticket}");
    println!("join:     {join}");
    match qrcode::QrCode::new(&join) {
        Ok(code) => {
            use qrcode::render::{svg, unicode::Dense1x2};
            // Terminals usually draw light glyphs on a dark background, so swap the
            // module colours; phone cameras often refuse inverted codes.
            println!(
                "{}",
                code.render::<Dense1x2>()
                    .dark_color(Dense1x2::Light)
                    .light_color(Dense1x2::Dark)
                    .quiet_zone(true)
                    .build()
            );
            let svg_path = data_dir.join("join-qr.svg");
            let image = code.render::<svg::Color>().min_dimensions(512, 512).build();
            match std::fs::write(&svg_path, image) {
                Ok(()) => println!("qr image: {}", svg_path.display()),
                Err(err) => tracing::warn!("could not write {}: {err}", svg_path.display()),
            }
        }
        Err(err) => tracing::warn!("could not render the join link as a QR code: {err}"),
    }

    tokio::signal::ctrl_c().await?;
    router.shutdown().await?;
    Ok(())
}

/// n0's public relays without the root dot. A host advertises its home relay
/// to peers, and a browser that dials the dotted name fails WebKit's
/// certificate check (`proto::without_root_dot`).
fn n0_relays() -> iroh::RelayMap {
    iroh::defaults::prod::default_relay_map()
        .urls::<Vec<_>>()
        .iter()
        .map(proto::without_root_dot)
        .collect()
}
