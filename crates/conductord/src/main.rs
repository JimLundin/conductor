//! `conductord`, the host daemon: the one place that names the providers.
//! It builds each provider's plugin from the command line and hands them
//! to the `daemon` library, which knows none by name.

use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use daemon::start::{Hosting, Plugins};
use proto::{InstanceId, Provider};
use runtime::Settings;

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
    /// Sign an instance's CLI in on this terminal, into the instance's
    /// private home: `claude auth login --claudeai` for a Claude
    /// subscription, `codex login --device-auth` for `ChatGPT` by device
    /// code. Until the app can sign in (docs/roadmap.md).
    Login(LoginArgs),
}

#[derive(Parser)]
struct LoginArgs {
    /// The instance to sign in, by its id: on a fresh host `claude` or
    /// `codex`, and otherwise as the app's credentials screen shows it.
    instance: String,
    /// The host's data dir, as given to `conductord host`.
    #[arg(long)]
    data_dir: Option<PathBuf>,
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
    /// A provider's CLI to run instead of the pinned one the host installs,
    /// for tests and development, such as `codex=/opt/codex`: the default
    /// of each of its instances. An instance where it doesn't run offers no
    /// sessions. Give it once for each provider.
    #[arg(long = "cli", value_name = "PROVIDER=PATH", value_parser = host::start::setting)]
    clis: Vec<(Provider, String)>,
    /// The model the sessions of a provider's instances get when they name
    /// none, such as `claude=haiku`. Defaults to the CLI's.
    #[arg(long = "model", value_name = "PROVIDER=MODEL", value_parser = host::start::setting)]
    models: Vec<(Provider, String)>,
}

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
    match Args::parse().command {
        Cmd::Host(args) => {
            let hosting = Hosting {
                data_dir: args.data_dir,
                name: args.name,
                web_url: args.web_url,
            };
            daemon::start::serve(hosting, plugins(args.clis, args.models)).await
        }
        Cmd::Login(args) => {
            let plugins = plugins(Vec::new(), Vec::new());
            let instance = InstanceId(args.instance);
            daemon::start::login(args.data_dir, plugins, instance).await
        }
    }
}

/// Every provider's plugin, with the settings the command line gave it.
fn plugins(
    given: Vec<(Provider, String)>,
    chosen: Vec<(Provider, String)>,
) -> Plugins {
    let paths = BTreeMap::from_iter(given);
    let models = BTreeMap::from_iter(chosen);
    let settings = |provider| Settings {
        path: paths.get(&provider).map(PathBuf::from),
        model: models.get(&provider).cloned(),
    };
    Plugins::from_iter([
        claude_cli::plugin(settings(Provider::Claude)),
        codex_cli::plugin(settings(Provider::Codex)),
    ])
}
