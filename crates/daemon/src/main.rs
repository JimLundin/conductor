use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use daemon::host::{Host, HostOptions};
use daemon::server::Conductor;
use iroh::Endpoint;
use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh_mdns_address_lookup::MdnsAddressLookup;
use proto::{AuthToken, Epoch, Ticket};

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
        } => {
            let data_dir = match data_dir {
                Some(dir) => dir,
                None => directories::ProjectDirs::from("", "", "conductor")
                    .context("no home directory to put the data dir in")?
                    .data_dir()
                    .to_path_buf(),
            };
            s3_host(data_dir, name, web_url, Duration::from_millis(delta_ms)).await
        }
    }
}

async fn s3_host(
    data_dir: PathBuf,
    name: String,
    web_url: String,
    delta_interval: Duration,
) -> anyhow::Result<()> {
    let key = daemon::load_or_create_key(&data_dir)?;
    let endpoint = Endpoint::builder(presets::N0)
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

    let token = AuthToken::from_bytes(daemon::random());
    let host = Host::spawn(HostOptions {
        name,
        epoch: Epoch(u64::from_le_bytes(daemon::random())),
        delta_interval,
    });
    let router = Router::builder(endpoint.clone())
        .accept(proto::ALPN, Conductor { host, token })
        .spawn();

    let ticket = Ticket {
        host: endpoint.id(),
        relay,
        token,
    };
    println!("data dir: {}", data_dir.display());
    println!("host id:  {}", endpoint.id());
    println!("relay:    {}", ticket.relay);
    println!("ticket:   {ticket}");
    println!("join:     {web_url}#join={ticket}");

    tokio::signal::ctrl_c().await?;
    router.shutdown().await?;
    Ok(())
}
