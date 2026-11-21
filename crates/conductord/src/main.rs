//! `conductord`, the host daemon: the one place that names the providers.
//! It builds each provider's plugin from the command line and hands them
//! to the `daemon` library, which knows none by name.

use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use daemon::{Hosting, Plugins};
use proto::Provider;
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
    /// Manage the local fleet root and its membership.
    Fleet(FleetArgs),
    /// Sign the public pairing request shown by a browser.
    Pair {
        request: String,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// Join a fleet using an invitation from its root host.
    Join {
        ticket: String,
        #[arg(long, default_value = "conductor")]
        name: String,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// Sign an instance's CLI in on this terminal, into the instance's
    /// private home: `claude auth login --claudeai` for a Claude
    /// subscription, `codex login --device-auth` for `ChatGPT` by device
    /// code. Until the app can sign in (docs/roadmap.md).
    Login(LoginArgs),
}

#[derive(Parser)]
struct FleetArgs {
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: FleetCommand,
}

#[derive(Subcommand)]
enum FleetCommand {
    Init {
        #[arg(long)]
        name: String,
        #[arg(long)]
        relay: String,
        #[arg(long)]
        backup: PathBuf,
    },
    Status,
    Revision,
    Revoke {
        node: String,
    },
    Restore {
        #[arg(long)]
        backup: PathBuf,
        #[arg(
            long,
            value_name = "FILE",
            required_unless_present = "initial_only"
        )]
        configuration: Option<PathBuf>,
        #[arg(long, conflicts_with = "configuration")]
        initial_only: bool,
    },
    Publish {
        revision: PathBuf,
    },
    ExportConfiguration {
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Parser)]
struct LoginArgs {
    /// The provider whose CLI signs in: `claude` or `codex`.
    #[arg(value_parser = host::start::provider)]
    provider: Provider,
    /// The account of the instance to sign in again, as the app's
    /// credentials screen shows it. Without it the provider's own sign-in
    /// runs, and the account the CLI names becomes an instance.
    #[arg(long)]
    account: Option<String>,
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
    /// The home's git remote (docs/home.md), such as
    /// `https://github.com/JimLundin/home.git`. The host clones it into its
    /// data dir and keeps it there; the host remembers it, so give it once.
    #[arg(long, value_name = "REMOTE")]
    home: Option<String>,
    /// A directory to look for the home's projects in: each directory in
    /// it and in those. Give it once for each.
    #[arg(long = "projects", value_name = "DIR")]
    projects: Vec<PathBuf>,
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
                home: args.home,
                projects: args.projects,
            };
            daemon::serve(hosting, plugins(args.clis, args.models)).await
        }
        Cmd::Fleet(args) => fleet(args).await,
        Cmd::Pair { request, data_dir } => {
            daemon::fleet::pair(data_dir, &request).await
        }
        Cmd::Join {
            ticket,
            name,
            data_dir,
        } => daemon::fleet::join(data_dir, &ticket, name).await,
        Cmd::Login(args) => {
            let plugins = plugins(Vec::new(), Vec::new());
            daemon::login(args.data_dir, plugins, args.provider, args.account)
                .await
        }
    }
}

async fn fleet(args: FleetArgs) -> anyhow::Result<()> {
    match args.command {
        FleetCommand::Init {
            name,
            relay,
            backup,
        } => {
            let passphrase = zeroize::Zeroizing::new(
                rpassword::prompt_password("Fleet backup passphrase: ")?,
            );
            let confirmation =
                zeroize::Zeroizing::new(rpassword::prompt_password(
                    "Confirm fleet backup passphrase: ",
                )?);
            anyhow::ensure!(
                passphrase.as_str() == confirmation.as_str(),
                "passphrases do not match"
            );
            daemon::fleet::initialize(
                args.data_dir,
                name,
                relay,
                &backup,
                passphrase.as_bytes(),
            )
        }
        FleetCommand::Status => daemon::fleet::status(args.data_dir).await,
        FleetCommand::Revision => daemon::fleet::revision(args.data_dir).await,
        FleetCommand::Revoke { node } => {
            daemon::fleet::revoke(args.data_dir, &node).await
        }
        FleetCommand::Publish { revision } => {
            daemon::fleet::publish(args.data_dir, &revision).await
        }
        FleetCommand::ExportConfiguration { output } => {
            let passphrase = zeroize::Zeroizing::new(
                rpassword::prompt_password("Fleet backup passphrase: ")?,
            );
            daemon::fleet::export(args.data_dir, &output, passphrase.as_bytes())
                .await
        }
        FleetCommand::Restore {
            backup,
            configuration,
            initial_only: _,
        } => {
            let passphrase = zeroize::Zeroizing::new(
                rpassword::prompt_password("Fleet backup passphrase: ")?,
            );
            daemon::fleet::restore(
                args.data_dir,
                &backup,
                configuration.as_deref(),
                passphrase.as_bytes(),
            )
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
    .with_fleet_credentials(fleet_sharing)
}

fn fleet_sharing(
    provider: Provider,
    credential: &proto::Credential,
) -> daemon::fleet::Sharing {
    use daemon::fleet::Sharing;
    match (provider, credential.kind.as_str()) {
        (Provider::Claude | Provider::Codex, "api_key") => Sharing::Shared,
        (Provider::Claude, "setup_token") => Sharing::OnePerProvider,
        _ => Sharing::Local,
    }
}
