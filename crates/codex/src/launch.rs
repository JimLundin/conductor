//! How the `codex app-server` starts, as data: its command line, its
//! environment and the provider in a private home's `config.toml` (§6.3).
//! Also the version its handshake reports and the paging through a
//! thread's history.

use agents::{HomeKind, Invocation, Var};
use proto::CodexCredential;

use crate::TurnId;
use crate::prelude::*;

/// Our provider's id in `model_providers`.
const PROVIDER_ID: &str = "conductor";
/// The variable our provider's `env_key` names, which carries its key.
pub const KEY_VAR: &str = "CONDUCTOR_CODEX_KEY";

/// `codex` with its home.
#[must_use]
pub fn cli(home: HomeKind) -> Invocation {
    let env = match home {
        HomeKind::User => Vec::new(),
        HomeKind::Private => Vec::from([
            Var::remove("CODEX_HOME"),
            Var::remove(KEY_VAR),
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

/// The app-server on stdio, with the key of `credential`'s provider in its
/// environment.
#[must_use]
pub fn app_server(
    home: HomeKind,
    credential: Option<&CodexCredential>,
) -> Invocation {
    let key = credential
        .map(|CodexCredential::Provider { key, .. }| Var::set(KEY_VAR, key));
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
/// `[model_providers]` above it. A key whose table on the way is not a
/// table is left alone: that is Codex's to refuse.
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
    let ours = match credential {
        Some(CodexCredential::Provider {
            name,
            base_url,
            key: _,
            model,
        }) => Vec::from([
            Edit::Table {
                path: path(&["model_providers", PROVIDER_ID]),
                entries: [
                    ("name", name.as_str()),
                    ("base_url", base_url.as_str()),
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
            Edit::Set {
                path: path(&["model"]),
                value: model.clone(),
            },
        ]),
        None => Vec::from([
            Edit::Remove(path(&["model_providers", PROVIDER_ID])),
            Edit::Remove(path(&[FILTERS[0], FILTERS[1], KEY_VAR])),
            // Ours was its only filter: a bare header would be left.
            Edit::RemoveIfEmpty(path(&FILTERS)),
        ]),
    };
    [
        Vec::from([
            Edit::Remove(path(&["model_provider"])),
            Edit::Remove(path(&["model"])),
        ]),
        ours,
    ]
    .concat()
}

/// The pages of a thread's history a check reads at most.
const PAGE_LIMIT: usize = 100;

/// A thread's native turns, read page by page.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct History {
    turns: Vec<TurnId>,
    pages: usize,
    cursor: Option<String>,
}

/// Where reading a history stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Paging {
    /// Fetch the page at the history's cursor.
    Fetch(History),
    /// Every turn, in chronological order.
    Done(Vec<TurnId>),
    /// The history runs past a hundred pages; a partial one would
    /// pass an undo check it should fail.
    Limit,
}

impl History {
    #[must_use]
    pub fn start() -> Paging {
        Paging::Fetch(Self::default())
    }

    /// Where the next page starts.
    #[must_use]
    pub fn cursor(&self) -> Option<String> {
        self.cursor.clone()
    }

    #[must_use]
    pub fn add(mut self, turns: Vec<TurnId>, cursor: Option<String>) -> Paging {
        self.turns.extend(turns);
        self.pages = self.pages.saturating_add(1);
        match (cursor, self.pages) {
            (None, _) => Paging::Done(self.turns),
            (Some(_), PAGE_LIMIT) => Paging::Limit,
            (next, _) => Paging::Fetch(Self {
                cursor: next,
                ..self
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use agents::{HomeKind, Invocation, Var};
    use proto::CodexCredential;

    use super::{
        Edit, History, Paging, app_server, config_edits, login, version,
    };
    use crate::TurnId;

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

    fn turns(ids: &[&str]) -> Vec<TurnId> {
        ids.iter().map(|id| TurnId(String::from(*id))).collect()
    }

    /// The page after `paging`, with `turns` and `cursor`.
    fn next(
        paging: Paging,
        turns: Vec<TurnId>,
        cursor: Option<&str>,
    ) -> Paging {
        match paging {
            Paging::Fetch(history) => {
                history.add(turns, cursor.map(String::from))
            }
            other => other,
        }
    }

    fn pages(count: usize) -> Paging {
        (0..count).fold(History::start(), |paging, _| {
            next(paging, turns(&["t"]), Some("more"))
        })
    }

    #[test]
    fn a_history_reads_pages_until_its_cursor_runs_out() {
        let second = next(History::start(), turns(&["t1", "t2"]), Some("c1"));
        let cursor = match &second {
            Paging::Fetch(history) => history.cursor(),
            Paging::Done(_) | Paging::Limit => None,
        };
        assert_eq!(
            (cursor, next(second, turns(&["t3"]), None)),
            (
                Some(String::from("c1")),
                Paging::Done(turns(&["t1", "t2", "t3"]))
            )
        );
    }

    #[test]
    fn a_history_past_a_hundred_pages_is_refused() {
        assert_eq!(
            (pages(100), next(pages(99), Vec::new(), None)),
            (Paging::Limit, Paging::Done(turns(&["t"; 99])))
        );
    }
}
