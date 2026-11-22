//! How the `codex app-server` starts, as data: its command line, its
//! environment and the provider in a private home's `config.toml` (§6.3).
//! Also the version its handshake reports.

use agents::pin::Cli;
use agents::{Config, HomeKind, Invocation, Profile, Reported, Var};
use proto::Provider;

use crate::CodexCredential;
use crate::prelude::*;

/// Our provider's id in `model_providers`.
const PROVIDER_ID: &str = "conductor";
/// The variable our provider's `env_key` names, which carries its key.
pub const KEY_VAR: &str = "CONDUCTOR_CODEX_KEY";
/// Where a signed-in home keeps its sign-in, which names its account.
const AUTH_FILE: &str = "auth.json";

/// `codex` with its home.
#[must_use]
pub fn cli(home: HomeKind) -> Invocation {
    let env = match home {
        HomeKind::User => Vec::new(),
        HomeKind::Private => Vec::from([
            Var::remove("CODEX_HOME"),
            Var::remove(KEY_VAR),
            Var::remove("OPENAI_API_KEY"),
            Var::remove("CODEX_API_KEY"),
            Var::remove("OPENAI_BASE_URL"),
            Var::Home(String::from("CODEX_HOME")),
        ]),
    };
    Invocation::of(env, &[])
}

/// `codex login status`, which succeeds when the home is signed in.
#[must_use]
pub fn login_status(home: HomeKind) -> Invocation {
    Invocation::of(cli(home).env, &["login", "status"])
}

/// The `ChatGPT` sign-in by device code, `codex login --device-auth`. It
/// stores the credential in its home itself.
#[must_use]
pub fn login(home: HomeKind) -> Invocation {
    Invocation::of(cli(home).env, &["login", "--device-auth"])
}

/// Codex as the host runs it, with its own update check off, since the
/// host installs the version it runs.
#[must_use]
pub fn profile() -> Profile {
    Profile {
        provider: Provider::Codex,
        info: crate::info(),
        pin: Cli::Codex,
        instructions_file: String::from("AGENTS.md"),
        config: Config {
            file: String::from("config.toml"),
            contents: String::from("check_for_update_on_startup = false\n"),
        },
        version: Invocation::of(cli(HomeKind::Private).env, &["--version"]),
        status: login_status(HomeKind::Private),
        account: Reported::File(String::from(AUTH_FILE)),
        sign_in: crate::sign_in(),
    }
}

/// The app-server on stdio, with the key of `credential`'s provider in its
/// environment.
#[must_use]
pub fn app_server(
    home: HomeKind,
    credential: Option<&CodexCredential>,
) -> Invocation {
    let key = credential.map(|stored| match stored {
        CodexCredential::ApiKey { key }
        | CodexCredential::Provider { key, .. } => Var::set(KEY_VAR, key),
    });
    Invocation::of(
        cli(home).env.into_iter().chain(key).collect(),
        &["app-server", "--listen", "stdio://"],
    )
}

/// The Codex version in a `userAgent`, whose first segment carries it, as
/// in `conductor/0.166.0 (...)`.
#[must_use]
pub fn version(user_agent: &str) -> Option<&str> {
    user_agent
        .split_whitespace()
        .next()?
        .split_once('/')
        .map(|(_, version)| version)
}

/// One change to a `config.toml`.
///
/// A path names the tables down to its key; a table on the way that is
/// missing is made with no header of its own, so
/// `[model_providers.conductor]` is written without a bare
/// `[model_providers]` above it. An incompatible parent table prevents
/// setting a key. On removal the key is already absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    Remove(Vec<String>),
    Set {
        path: Vec<String>,
        value: String,
    },
    /// A table of these strings, in order.
    Table {
        path: Vec<String>,
        entries: Vec<(String, String)>,
    },
    /// Removes the table at `path` if it has nothing in it.
    RemoveIfEmpty(Vec<String>),
}

fn path(keys: &[&str]) -> Vec<String> {
    keys.iter().copied().map(String::from).collect()
}

/// `shell_environment_policy.filters`, where our exclude keeps the key out
/// of every command Codex runs. Codex 0.166 passes variables named like a
/// key to them by default (`ignore_default_excludes` defaults to true in
/// `config/src/shell_environment_policy.rs`).
const FILTERS: [&str; 2] = ["shell_environment_policy", "filters"];

/// What puts `credential`'s provider into a private home's `config.toml`,
/// or takes ours out for `None`.
///
/// The home's `model_provider`, `model` and
/// `[model_providers.conductor]` are Conductor's; everything else Codex
/// wrote there stays. The key stays out of the file.
#[must_use]
pub fn config_edits(credential: Option<&CodexCredential>) -> Vec<Edit> {
    let base = Vec::from([
        Edit::Remove(path(&["model_provider"])),
        Edit::Remove(path(&["model"])),
    ]);
    let ours = match credential {
        Some(CodexCredential::Provider {
            name,
            base_url,
            model,
            ..
        }) => {
            let mut edits = provider_edits(name, base_url);
            edits.push(Edit::Set {
                path: path(&["model"]),
                value: model.clone(),
            });
            edits
        }
        Some(CodexCredential::ApiKey { .. }) => {
            provider_edits("OpenAI", "https://api.openai.com/v1")
        }
        None => Vec::from([
            Edit::Remove(path(&["model_providers", PROVIDER_ID])),
            Edit::Remove(path(&[FILTERS[0], FILTERS[1], KEY_VAR])),
            // Ours was its only filter: a bare header would be left.
            Edit::RemoveIfEmpty(path(&FILTERS)),
        ]),
    };
    [base, ours].concat()
}

fn provider_edits(name: &str, base_url: &str) -> Vec<Edit> {
    Vec::from([
        Edit::Table {
            path: path(&["model_providers", PROVIDER_ID]),
            entries: [
                ("name", name),
                ("base_url", base_url),
                ("env_key", KEY_VAR),
                ("wire_api", "responses"),
            ]
            .map(|(key, value)| (String::from(key), String::from(value)))
            .into(),
        },
        Edit::Set {
            path: path(&[FILTERS[0], FILTERS[1], KEY_VAR]),
            value: String::from("exclude"),
        },
        Edit::Set {
            path: path(&["model_provider"]),
            value: String::from(PROVIDER_ID),
        },
    ])
}

#[cfg(test)]
mod tests {
    use agents::{HomeKind, Invocation, Var};
    use proto::Provider;

    use super::{Edit, app_server, cli, config_edits, login, profile, version};
    use crate::CodexCredential;

    #[test]
    fn the_profile_runs_codex_in_its_private_home() {
        let profile = profile();
        let private = cli(HomeKind::Private).env;
        assert_eq!(
            (
                profile.provider,
                profile.config.file.as_str(),
                [profile.version, profile.status, profile.sign_in.run]
            ),
            (
                Provider::Codex,
                "config.toml",
                [
                    Invocation::of(private.clone(), &["--version"]),
                    Invocation::of(private.clone(), &["login", "status"]),
                    Invocation::of(private, &["login", "--device-auth"]),
                ]
            )
        );
    }

    fn provider() -> CodexCredential {
        CodexCredential::Provider {
            name: String::from("Gateway"),
            base_url: String::from("https://gateway.example/v1"),
            key: String::from("sk-1"),
            model: String::from("gpt-6"),
        }
    }

    fn path(keys: &[&str]) -> Vec<String> {
        keys.iter().copied().map(String::from).collect()
    }

    #[test]
    fn a_private_home_replaces_codex_home_and_the_key_is_in_the_environment() {
        assert_eq!(
            (
                app_server(HomeKind::Private, Some(&provider())),
                app_server(HomeKind::User, None),
                login(HomeKind::User).args,
            ),
            (
                Invocation {
                    env: Vec::from([
                        Var::remove("CODEX_HOME"),
                        Var::remove("CONDUCTOR_CODEX_KEY"),
                        Var::remove("OPENAI_API_KEY"),
                        Var::remove("CODEX_API_KEY"),
                        Var::remove("OPENAI_BASE_URL"),
                        Var::Home(String::from("CODEX_HOME")),
                        Var::set("CONDUCTOR_CODEX_KEY", "sk-1"),
                    ]),
                    args: path(&["app-server", "--listen", "stdio://"]),
                },
                Invocation {
                    env: Vec::new(),
                    args: path(&["app-server", "--listen", "stdio://"]),
                },
                path(&["login", "--device-auth"]),
            )
        );
    }

    #[test]
    fn a_direct_key_uses_the_filtered_provider_environment() {
        let credential = CodexCredential::ApiKey {
            key: String::from("synthetic-api-key"),
        };
        let invocation = app_server(HomeKind::Private, Some(&credential));
        assert_eq!(
            invocation.env.last(),
            Some(&Var::set("CONDUCTOR_CODEX_KEY", "synthetic-api-key"))
        );
        assert!(
            invocation
                .args
                .iter()
                .all(|arg| !arg.contains("synthetic-api-key"))
        );
        assert_eq!(crate::default_model(&credential), None);
    }

    #[test]
    fn the_version_is_the_first_segments_after_its_slash() {
        assert_eq!(
            [
                version("conductor/0.166.0 (Ubuntu 24.4.0; x86_64)"),
                version("conductor"),
                version(""),
            ],
            [Some("0.166.0"), None, None]
        );
    }

    #[test]
    fn the_provider_goes_in_without_its_key_and_comes_out_with_its_filter() {
        let set = config_edits(Some(&provider()));
        let unset = config_edits(None);
        assert_eq!(
            (&set[2..], &unset[2..]),
            (
                &[
                    Edit::Table {
                        path: path(&["model_providers", "conductor"]),
                        entries: [
                            ("name", "Gateway"),
                            ("base_url", "https://gateway.example/v1"),
                            ("env_key", "CONDUCTOR_CODEX_KEY"),
                            ("wire_api", "responses"),
                        ]
                        .map(|(key, value)| {
                            (String::from(key), String::from(value))
                        })
                        .into(),
                    },
                    Edit::Set {
                        path: path(&[
                            "shell_environment_policy",
                            "filters",
                            "CONDUCTOR_CODEX_KEY"
                        ]),
                        value: String::from("exclude"),
                    },
                    Edit::Set {
                        path: path(&["model_provider"]),
                        value: String::from("conductor"),
                    },
                    Edit::Set {
                        path: path(&["model"]),
                        value: String::from("gpt-6"),
                    },
                ][..],
                &[
                    Edit::Remove(path(&["model_providers", "conductor"])),
                    Edit::Remove(path(&[
                        "shell_environment_policy",
                        "filters",
                        "CONDUCTOR_CODEX_KEY"
                    ])),
                    Edit::RemoveIfEmpty(path(&[
                        "shell_environment_policy",
                        "filters"
                    ])),
                ][..],
            )
        );
    }
}
