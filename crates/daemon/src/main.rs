use clap::Parser;
use daemon::start::Args;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| {
                    "info,iroh=warn,noq=warn,swarm_discovery=warn".into()
                }),
        )
        .with_writer(std::io::stderr)
        .init();
    daemon::start::run(Args::parse()).await
}
