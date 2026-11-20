//! How the host starts each provider's CLI, one step at a time.
//!
//! It runs the CLI it was given, or else the pinned one it installs; it
//! offers the CLI only if `--version` succeeds; a pinned CLI signs in on its
//! own only if its private home passes the CLI's sign-in check; and it
//! offers what the CLI lists, with the model a session gets when it names
//! none.
//!
//! `daemon` performs each [`Next`]: it installs, runs the probes and builds
//! the provider instance `S`. The CLI's path `P` is opaque here.

use alloc::format;

use agents::Configuration;

use proto::{ModelId, Models, Provider};

use crate::Note;
use crate::prelude::*;
use crate::registry::{Launcher, OwnSignIn, offered};

/// The provider the host's command line calls `name`.
///
/// # Errors
/// No provider has that name.
pub fn provider(name: &str) -> Result<Provider, String> {
    Provider::named(name).ok_or_else(|| {
        let names = Provider::ALL.map(Provider::name).join(", ");
        format!("no provider is called {name:?}; the providers are {names}")
    })
}

/// A provider's setting on the host's command line, `<provider>=<value>`.
///
/// # Errors
/// The text has no `=`, or no provider has the name before it.
pub fn setting(text: &str) -> Result<(Provider, String), String> {
    let (name, value) = text
        .split_once('=')
        .ok_or_else(|| format!("{text:?} is not <provider>=<value>"))?;
    Ok((provider(name)?, String::from(value)))
}

/// What the shell does next, and the step that takes its outcome.
#[derive(Debug)]
pub enum Next<P, S> {
    /// Install the pinned CLI, and hand its path to [`Installing::installed`].
    Install(Installing),
    /// Build the instance for the CLI at `P`, run `P --version`, and hand
    /// both to [`Probing::probed`].
    Probe(P, Probing),
    /// Run the sign-in check of the CLI at `P`, and hand how it exited to
    /// [`Checking::checked`].
    CheckSignIn(P, Checking<S>),
    /// Apply the instance's stored credential and discover its models.
    Configure(Configuring<S>),
    /// Run the provider as the launcher says, after logging the notes.
    Done(Launcher<S>, Vec<Note>),
}

/// How a probe of the CLI exited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub code: Option<i32>,
    pub stdout: String,
}

/// Starts the CLI called `name`: the `given` one, or else the pinned one.
/// A session that names no model gets the one the stored credential names,
/// else `chosen`, the one the host was started with.
#[must_use]
pub fn start<P, S>(
    name: &str,
    given: Option<P>,
    chosen: Option<String>,
) -> Next<P, S> {
    match given {
        Some(path) => Next::Probe(
            path,
            Probing {
                pinned: false,
                chosen,
            },
        ),
        None => Next::Install(Installing {
            name: String::from(name),
            chosen,
        }),
    }
}

fn succeeded(code: Option<i32>) -> bool {
    code == Some(0)
}

/// The pinned CLI is being installed.
#[derive(Debug)]
pub struct Installing {
    name: String,
    chosen: Option<String>,
}

impl Installing {
    #[must_use]
    pub fn installed<P, S>(self, path: Result<P, String>) -> Next<P, S> {
        match path {
            Ok(installed) => Next::Probe(
                installed,
                Probing {
                    pinned: true,
                    chosen: self.chosen,
                },
            ),
            Err(err) => unavailable(format!(
                "installing the pinned {}: {err}",
                self.name
            )),
        }
    }
}

/// The CLI's `--version` is running.
#[derive(Debug)]
pub struct Probing {
    pinned: bool,
    chosen: Option<String>,
}

impl Probing {
    #[must_use]
    pub fn probed<P, S>(
        self,
        path: P,
        start: S,
        program: String,
        ran: Result<Output, String>,
    ) -> Next<P, S> {
        let version = match ran {
            Ok(output) if succeeded(output.code) => {
                String::from(output.stdout.trim())
            }
            Ok(output) => {
                return unavailable(format!(
                    "{program} --version exited with {:?}",
                    output.code
                ));
            }
            Err(err) => {
                return unavailable(format!("{program} does not run: {err}"));
            }
        };
        let cli = Cli {
            path: program,
            version,
            start,
            chosen: self.chosen,
        };
        if self.pinned {
            Next::CheckSignIn(path, Checking { cli })
        } else {
            Next::Configure(Configuring {
                cli,
                own: OwnSignIn::Unmanaged,
            })
        }
    }
}

/// A CLI that runs.
#[derive(Debug)]
struct Cli<S> {
    path: String,
    version: String,
    start: S,
    chosen: Option<String>,
}

/// The pinned CLI's sign-in check is running against its private home.
#[derive(Debug)]
pub struct Checking<S> {
    cli: Cli<S>,
}

impl<S> Checking<S> {
    /// The check exited with `code`, or did not finish, for instance in
    /// time.
    #[must_use]
    pub fn checked<P>(self, code: &Result<Option<i32>, String>) -> Next<P, S> {
        let own = match code {
            Ok(exit) if succeeded(*exit) => OwnSignIn::SignedIn,
            Ok(_) | Err(_) => OwnSignIn::Missing,
        };
        Next::Configure(Configuring { cli: self.cli, own })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Boot,
    Change,
}

#[derive(Debug)]
pub struct Configuring<S> {
    cli: Cli<S>,
    own: OwnSignIn,
}

impl<S> Configuring<S> {
    #[must_use]
    pub const fn instance(&self) -> &S {
        &self.cli.start
    }

    #[must_use]
    pub fn configured<P>(
        self,
        result: Result<Configuration, String>,
        mode: Mode,
    ) -> Next<P, S> {
        let Self { cli, own } = self;
        let chosen = cli.chosen.map(ModelId);
        let mut notes = Vec::new();
        let models = result
            .map_err(|reason| {
                format!("{} refused the stored credential: {reason}", cli.path)
            })
            .and_then(|mut config| {
                if mode == Mode::Boot
                    && let Err(reason) = config.models
                {
                    notes.push(Note::NoModels { reason });
                    config.models = Ok(Models::none());
                }
                offered(config, chosen.as_ref())
            });
        notes.push(match &models {
            Ok(list) => Note::Ready {
                path: cli.path,
                version: cli.version,
                own,
                models: list.choices.len(),
            },
            Err(reason) => Note::Unavailable {
                reason: reason.clone(),
            },
        });
        Next::Done(
            Launcher::Running {
                start: cli.start,
                own,
                chosen,
                models,
            },
            notes,
        )
    }
}

fn unavailable<P, S>(reason: String) -> Next<P, S> {
    let note = Note::Unavailable {
        reason: reason.clone(),
    };
    Next::Done(Launcher::Unavailable(reason), Vec::from([note]))
}

#[cfg(test)]
mod tests {
    use proto::{Effort, ModelChoice, ModelId, Models, Provider};

    use super::{Mode, Next, Output, setting, start};
    use crate::Note;
    use crate::registry::{Launcher, OwnSignIn};

    type Step = Next<&'static str, &'static str>;

    #[test]
    fn a_setting_names_its_provider_before_the_first_equals_sign() {
        assert_eq!(
            ["codex=a=b", "claude=", "claude", "gemini=pro"].map(setting),
            [
                Ok((Provider::Codex, String::from("a=b"))),
                Ok((Provider::Claude, String::new())),
                Err(String::from(r#""claude" is not <provider>=<value>"#)),
                Err(String::from(
                    r#"no provider is called "gemini"; the providers are claude, codex"#
                )),
            ]
        );
    }

    fn ran(code: i32) -> Output {
        Output {
            code: Some(code),
            stdout: String::from("1.2.3 (Claude Code)\n"),
        }
    }

    fn listed() -> Models {
        Models {
            default: Some(ModelId(String::from("opus"))),
            choices: Vec::from([ModelChoice {
                id: ModelId(String::from("opus")),
                name: String::from("Opus"),
                description: String::new(),
                efforts: Vec::from([Effort::High]),
            }]),
        }
    }

    /// What each probe of a startup says.
    struct Probes {
        installed: Result<&'static str, String>,
        version: Result<Output, String>,
        check: Result<Option<i32>, String>,
        named: Result<Option<ModelId>, String>,
        models: Result<Models, String>,
    }

    fn probes() -> Probes {
        Probes {
            installed: Ok("/pinned"),
            version: Ok(ran(0)),
            check: Ok(Some(0)),
            named: Ok(Some(ModelId(String::from("named")))),
            models: Ok(listed()),
        }
    }

    /// Walks a startup through every step.
    fn walk(
        given: Option<&'static str>,
        probes: &Probes,
    ) -> (Vec<&'static str>, Launcher<&'static str>, Vec<Note>) {
        let mut seen = Vec::new();
        let mut next: Step =
            start("claude", given, Some(String::from("haiku")));
        loop {
            next = match next {
                Next::Install(installing) => {
                    seen.push("install");
                    installing.installed(probes.installed.clone())
                }
                Next::Probe(path, probing) => {
                    seen.push(path);
                    probing.probed(
                        path,
                        "start",
                        String::from(path),
                        probes.version.clone(),
                    )
                }
                Next::CheckSignIn(_, checking) => {
                    seen.push("check");
                    checking.checked(&probes.check)
                }
                Next::Configure(listing) => {
                    seen.push(listing.instance());
                    listing.configured(
                        probes.named.clone().map(|named| {
                            agents::Configuration {
                                named,
                                models: probes.models.clone(),
                            }
                        }),
                        Mode::Boot,
                    )
                }
                Next::Done(launcher, notes) => break (seen, launcher, notes),
            }
        }
    }

    fn ready(own: OwnSignIn, mut listed: Models) -> Launcher<&'static str> {
        listed.default = Some(ModelId(String::from("named")));
        Launcher::Running {
            start: "start",
            own,
            chosen: Some(ModelId(String::from("haiku"))),
            models: Ok(listed),
        }
    }

    #[test]
    fn a_given_cli_runs_as_it_is_signed_in() {
        assert_eq!(
            walk(Some("/given"), &probes()),
            (
                Vec::from(["/given", "start"]),
                ready(OwnSignIn::Unmanaged, listed()),
                Vec::from([Note::Ready {
                    path: String::from("/given"),
                    version: String::from("1.2.3 (Claude Code)"),
                    own: OwnSignIn::Unmanaged,
                    models: 1,
                }]),
            )
        );
    }

    #[test]
    fn a_pinned_cli_is_installed_and_signs_in_if_its_home_passes_the_check() {
        let pinned = |check| {
            let (seen, launcher, _) = walk(None, &Probes { check, ..probes() });
            (seen, launcher)
        };
        let steps = Vec::from(["install", "/pinned", "check", "start"]);
        assert_eq!(
            [
                pinned(Ok(Some(0))),
                pinned(Ok(Some(1))),
                pinned(Ok(None)),
                pinned(Err(String::from("timed out"))),
            ],
            [
                (steps.clone(), ready(OwnSignIn::SignedIn, listed())),
                (steps.clone(), ready(OwnSignIn::Missing, listed())),
                (steps.clone(), ready(OwnSignIn::Missing, listed())),
                (steps, ready(OwnSignIn::Missing, listed())),
            ]
        );
    }

    #[test]
    fn agents_that_refuse_the_stored_credential_are_unavailable() {
        let (_, launcher, _) = walk(
            Some("/given"),
            &Probes {
                named: Err(String::from("bad key")),
                ..probes()
            },
        );
        assert_eq!(
            launcher,
            Launcher::Running {
                start: "start",
                own: OwnSignIn::Unmanaged,
                chosen: Some(ModelId(String::from("haiku"))),
                models: Err(String::from(
                    "/given refused the stored credential: bad key"
                )),
            }
        );
    }

    #[test]
    fn a_cli_that_lists_nothing_offers_only_the_defaults() {
        let (_, launcher, notes) = walk(
            Some("/given"),
            &Probes {
                models: Err(String::from("not signed in")),
                ..probes()
            },
        );
        assert_eq!(
            (launcher, notes),
            (
                ready(OwnSignIn::Unmanaged, Models::none()),
                Vec::from([
                    Note::NoModels {
                        reason: String::from("not signed in")
                    },
                    Note::Ready {
                        path: String::from("/given"),
                        version: String::from("1.2.3 (Claude Code)"),
                        own: OwnSignIn::Unmanaged,
                        models: 0,
                    },
                ])
            )
        );
    }

    #[test]
    fn a_cli_that_does_not_install_or_run_is_unavailable() {
        let unavailable = |installed, version| {
            let (seen, launcher, notes) = walk(
                None,
                &Probes {
                    installed,
                    version,
                    ..probes()
                },
            );
            (seen.len(), launcher, notes)
        };
        let after = |steps: usize, reason: &str| {
            (
                steps,
                Launcher::Unavailable(String::from(reason)),
                Vec::from([Note::Unavailable {
                    reason: String::from(reason),
                }]),
            )
        };
        assert_eq!(
            [
                unavailable(Err(String::from("offline")), Ok(ran(0))),
                unavailable(Ok("/pinned"), Ok(ran(2))),
                unavailable(Ok("/pinned"), Err(String::from("no such file"))),
            ],
            [
                after(1, "installing the pinned claude: offline"),
                after(2, "/pinned --version exited with Some(2)"),
                after(2, "/pinned does not run: no such file"),
            ]
        );
    }
}
