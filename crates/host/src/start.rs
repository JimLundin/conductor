//! How the host starts each provider's CLI, one step at a time.
//!
//! It runs the CLI it was given, or else the pinned one it installs; it
//! offers the CLI only if `--version` succeeds; a pinned CLI signs in on its
//! own only if its private home passes the CLI's sign-in check; and it
//! offers what the CLI lists, with the model a session gets when it names
//! none.
//!
//! `daemon` performs each [`Next`]: it installs, runs the probes and builds
//! the provider's starter `S`. The CLI's path `P` is opaque here.

use alloc::format;

use proto::{ModelId, Models, Provider};

use crate::Note;
use crate::prelude::*;
use crate::registry::{Launcher, Offer, OwnSignIn};

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
    /// Build the starter for the CLI at `P`, run `P --version`, and hand
    /// both to [`Probing::probed`].
    Probe(P, Probing),
    /// Run the sign-in check of the CLI at `P`, and hand how it exited to
    /// [`Checking::checked`].
    CheckSignIn(P, Checking<S>),
    /// Hand [`Listing::start`]'s agents the stored credentials and list
    /// what it then offers, and hand both outcomes to [`Listing::listed`].
    ListModels(Listing<S>),
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

/// Whether a sign-in that exited as `status` with `code` succeeded.
///
/// # Errors
/// The sign-in failed.
pub fn signed_in(code: Option<i32>, status: &str) -> Result<(), String> {
    if succeeded(code) {
        Ok(())
    } else {
        Err(format!("the sign-in exited with {status}"))
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
    /// `program`'s `--version` `ran`; `start` starts its agents.
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
            Next::ListModels(Listing {
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
        Next::ListModels(Listing { cli: self.cli, own })
    }
}

/// The CLI is listing its models.
#[derive(Debug)]
pub struct Listing<S> {
    cli: Cli<S>,
    own: OwnSignIn,
}

impl<S> Listing<S> {
    #[must_use]
    pub const fn start(&self) -> &S {
        &self.cli.start
    }

    /// The agents were `configured` with the stored credentials, which name
    /// a default model or not, and the CLI offers what it then `listed`. One that
    /// could not list them, for instance because it is not signed in yet,
    /// offers only the default. Agents that refused the credentials can't
    /// run.
    #[must_use]
    pub fn listed<P>(
        self,
        configured: Result<Option<ModelId>, String>,
        listed: Result<Models, String>,
    ) -> Next<P, S> {
        let Self { cli, own } = self;
        let named = match configured {
            Ok(model) => model,
            Err(reason) => {
                return unavailable(format!(
                    "{} refused the stored credential: {reason}",
                    cli.path
                ));
            }
        };
        let (models, mut notes) = match listed {
            Ok(models) => (models, Vec::new()),
            Err(reason) => {
                (Models::none(), Vec::from([Note::NoModels { reason }]))
            }
        };
        notes.push(Note::Ready {
            path: cli.path,
            version: cli.version,
            own,
            models: models.choices.len(),
        });
        Next::Done(
            Launcher::Ready {
                start: cli.start,
                own,
                offer: Offer {
                    listed: models,
                    named,
                    chosen: cli.chosen.map(ModelId),
                },
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

    use super::{Next, Output, setting, signed_in, start};
    use crate::Note;
    use crate::registry::{Launcher, Offer, OwnSignIn};

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
                Next::ListModels(listing) => {
                    seen.push(listing.start());
                    listing.listed(probes.named.clone(), probes.models.clone())
                }
                Next::Done(launcher, notes) => break (seen, launcher, notes),
            }
        }
    }

    fn ready(own: OwnSignIn, listed: Models) -> Launcher<&'static str> {
        Launcher::Ready {
            start: "start",
            own,
            offer: Offer {
                listed,
                named: Some(ModelId(String::from("named"))),
                chosen: Some(ModelId(String::from("haiku"))),
            },
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
            Launcher::Unavailable(String::from(
                "/given refused the stored credential: bad key"
            ))
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

    #[test]
    fn a_sign_in_succeeds_only_when_it_exits_zero() {
        assert_eq!(
            [
                signed_in(Some(0), "exit status: 0"),
                signed_in(Some(1), "exit status: 1"),
                signed_in(None, "signal: 9"),
            ],
            [
                Ok(()),
                Err(String::from("the sign-in exited with exit status: 1")),
                Err(String::from("the sign-in exited with signal: 9")),
            ]
        );
    }
}
