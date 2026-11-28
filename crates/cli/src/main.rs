use std::collections::BTreeMap;
use std::path::PathBuf;

use agents::Provider;
use clap::{Parser, Subcommand};
use daemon::{Hosting, Plugins};
use runtime::Settings;

const PROVIDERS: [Provider; 2] = [claude::PROVIDER, codex::PROVIDER];

fn provider(name: &str) -> Result<Provider, String> {
    host::start::provider(name, &PROVIDERS)
}

fn setting(text: &str) -> Result<(Provider, String), String> {
    host::start::setting(text, &PROVIDERS)
}

#[derive(Parser)]
#[command(name = "orchestrator", about = "Run agent sessions and manage hosts")]
struct Args {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    #[command(
        about = "Serve this host's sessions over iroh and print a join ticket"
    )]
    Host(HostArgs),
    #[command(about = "Inspect a project in this host's home checkout")]
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    #[command(about = "Manage the local fleet root and its membership")]
    Fleet(FleetArgs),
    #[command(about = "Sign the public pairing request shown by a browser")]
    Pair {
        request: String,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    #[command(about = "Join a fleet using an invitation from its root host")]
    Join {
        ticket: String,
        #[arg(long, default_value = "conductor")]
        name: String,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    #[command(
        about = "Sign an instance's CLI in on this terminal, into the instance's private home: `claude auth login --claudeai` for a Claude subscription, `codex login --device-auth` for `ChatGPT` by device code. Until the app can sign in (docs/roadmap.md)"
    )]
    Login(LoginArgs),
}

#[derive(Subcommand)]
enum ProjectCommand {
    #[command(
        about = "Check GitHub repository access with this host's configured credential"
    )]
    Access {
        project: String,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
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
    #[command(about = "Manage project credentials shared with fleet hosts")]
    Credential {
        #[command(subcommand)]
        command: FleetCredential,
    },
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
    ExportConfiguration {
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Subcommand)]
enum FleetCredential {
    #[command(
        about = "Publish a named project credential from the root host's credential store"
    )]
    Share { reference: String },
    #[command(about = "Remove a shared project credential from the fleet")]
    Remove { reference: String },
}

#[derive(Parser)]
struct LoginArgs {
    #[arg(help = "The provider whose CLI signs in: `claude` or `codex`")]
    #[arg(value_parser = provider)]
    provider: Provider,
    #[arg(
        help = "The account of the instance to sign in again, as the app's credentials screen shows it. Without it the provider's own sign-in runs, and the account the CLI names becomes an instance"
    )]
    #[arg(long)]
    account: Option<String>,
    #[arg(help = "The host's data dir, as given to `orchestrator host`")]
    #[arg(long)]
    data_dir: Option<PathBuf>,
}

#[derive(Parser)]
struct HostArgs {
    #[arg(
        help = "Where the host key lives. Defaults to the OS data dir for \"conductor\""
    )]
    #[arg(long)]
    data_dir: Option<PathBuf>,
    #[arg(help = "The name clients show for this host")]
    #[arg(long, default_value = "conductor")]
    name: String,
    #[arg(
        help = "The web app's URL; the printed join link is this plus `#join=<ticket>`"
    )]
    #[arg(long, default_value = "http://localhost:8080/conductor/")]
    web_url: String,
    #[arg(
        help = "A provider's CLI to run instead of the pinned one the host installs, for tests and development, such as `codex=/opt/codex`: the default of each of its instances. An instance where it doesn't run offers no sessions. Give it once for each provider"
    )]
    #[arg(long = "cli", value_name = "PROVIDER=PATH", value_parser = setting)]
    clis: Vec<(Provider, String)>,
    #[arg(
        help = "The model the sessions of a provider's instances get when they name none, such as `claude=haiku`. Defaults to the CLI's"
    )]
    #[arg(long = "model", value_name = "PROVIDER=MODEL", value_parser = setting)]
    models: Vec<(Provider, String)>,
    #[arg(
        help = "The home's git remote (docs/home.md), such as `https://github.com/JimLundin/home.git`. The host clones it into its data dir and keeps it there; the host remembers it, so give it once"
    )]
    #[arg(long, value_name = "REMOTE")]
    home: Option<String>,
    #[arg(
        help = "A directory to look for the home's projects in: each directory in it and in those. Give it once for each"
    )]
    #[arg(long = "projects", value_name = "DIR")]
    projects: Vec<PathBuf>,
}

fn canvases(
    data: &std::path::Path,
    home: &home::flow::snapshot::Snapshot,
    project: &home::project::Project,
) -> Option<home_store::Kept> {
    github_api::canvases(data, home, project, daemon::fleet::project_credential)
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
            let data = project_runtime::data_dir_or_default(args.data_dir)?;
            let root = std::path::absolute(&data)?;
            let workspaces = daemon::Workspaces::new(
                vcs_git2::registration::local(&root),
                vcs_git2::registration::register,
            );
            let hosting = Hosting {
                canvases,
                data_dir: Some(data),
                name: args.name,
                web_url: args.web_url,
                home: args.home,
                projects: args.projects,
            };
            daemon::serve(hosting, plugins(args.clis, args.models), workspaces)
                .await
        }
        Cmd::Project {
            command: ProjectCommand::Access { project, data_dir },
        } => {
            let data = project_runtime::data_dir_or_default(data_dir)?;
            let (report, code) = github_api::project_access_with_fleet(
                &daemon::home_checkout(&data),
                &project,
                &data,
                daemon::fleet::project_credential,
            )
            .await
            .map_err(anyhow::Error::msg)?;
            std::io::Write::write_all(
                &mut std::io::stdout().lock(),
                report.as_bytes(),
            )?;
            anyhow::ensure!(code == 0, "some repository reads are unavailable");
            Ok(())
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
        FleetCommand::Credential { command } => {
            let data = project_runtime::data_dir_or_default(args.data_dir)?;
            let (reference, entry) = match command {
                FleetCredential::Share { reference } => {
                    let source = github_api::HostCredentials::new(&data);
                    let entry = source.export_for_fleet(&reference)?;
                    (reference, Some(entry))
                }
                FleetCredential::Remove { reference } => (reference, None),
            };
            daemon::fleet::change_project_credential(&data, reference, entry)
                .await
        }
        FleetCommand::Status => daemon::fleet::status(args.data_dir).await,
        FleetCommand::Revoke { node } => {
            daemon::fleet::revoke(args.data_dir, &node).await
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
        claude_cli::plugin(settings(claude::PROVIDER)),
        codex_cli::plugin(settings(codex::PROVIDER)),
    ])
    .with_fleet_credentials(fleet_sharing)
}

fn fleet_sharing(
    _: &Provider,
    credential: &agents::Credential,
) -> daemon::fleet::Sharing {
    use daemon::fleet::Sharing;
    match credential.kind.as_str() {
        "api_key" => Sharing::Shared,
        claude::SETUP_TOKEN => Sharing::OnePerProvider,
        _ => Sharing::Local,
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::Args;

    #[test]
    fn an_unknown_provider_is_refused_naming_the_ones_the_host_has() {
        let refusal = r#"no provider is called "gemini"; the providers are claude, codex"#;
        for given in [
            ["orchestrator", "host", "--cli", "gemini=/opt/gemini"],
            ["orchestrator", "host", "--model", "gemini=pro"],
            ["orchestrator", "login", "gemini", "--account=me"],
        ] {
            assert!(
                Args::try_parse_from(given)
                    .is_err_and(|error| error.to_string().contains(refusal)),
                "{given:?}"
            );
        }
        assert!(
            Args::try_parse_from(["orchestrator", "host", "--cli", "codex=/x"])
                .is_ok()
        );
    }

    #[test]
    fn fleet_commands_exclude_release_management() {
        assert!(
            Args::try_parse_from(["orchestrator", "fleet", "status"]).is_ok()
        );
        assert!(
            Args::try_parse_from(["orchestrator", "fleet", "revision"])
                .is_err()
        );
        assert!(
            Args::try_parse_from([
                "orchestrator",
                "fleet",
                "publish",
                "release.json"
            ])
            .is_err()
        );
    }
}
