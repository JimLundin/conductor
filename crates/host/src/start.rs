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

use agents::Note;
use proto::{ModelId, Models};

use crate::prelude::*;
use crate::registry::{Launcher, Offer, OwnSignIn};

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
    /// List what [`Listing::start`] offers, and hand it to
    /// [`Listing::listed`].
    ListModels(Listing<S>),
    /// Run the provider as the launcher says, after logging the notes.
    Done(Launcher<S>, Vec<Note>),
}

/// The defaults a session gets when it names no model: the one the stored
/// credential `named`, then the one the host was started with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Defaults {
    pub named: Option<ModelId>,
    pub chosen: Option<String>,
}

/// How a probe of the CLI exited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub code: Option<i32>,
    pub stdout: String,
}

/// Starts the CLI called `name`: the `given` one, or else the pinned one.
#[must_use]
pub fn start<P, S>(
    name: &str,
    given: Option<P>,
    defaults: Defaults,
) -> Next<P, S> {
    match given {
        Some(path) => Next::Probe(
            path,
            Probing {
                pinned: false,
                defaults,
            },
        ),
        None => Next::Install(Installing {
            name: String::from(name),
            defaults,
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
    defaults: Defaults,
}

impl Installing {
    #[must_use]
    pub fn installed<P, S>(self, path: Result<P, String>) -> Next<P, S> {
        match path {
            Ok(installed) => Next::Probe(
                installed,
                Probing {
                    pinned: true,
                    defaults: self.defaults,
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
    defaults: Defaults,
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
            defaults: self.defaults,
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
    defaults: Defaults,
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

    /// The CLI offers what it `listed`. One that could not list them, for
    /// instance because it is not signed in yet, offers only the default.
    #[must_use]
    pub fn listed<P>(self, listed: Result<Models, String>) -> Next<P, S> {
        let Self { cli, own } = self;
        let (models, mut notes) = match listed {
            Ok(models) => (models, Vec::new()),
            Err(reason) => (
                Models::none(),
                Vec::from([Note::warn(format!(
                    "the CLI listed no models: {reason}"
                ))]),
            ),
        };
        notes.push(Note::info(format!(
            "ready: {} {}, signed in {own:?}, {} models",
            cli.path,
            cli.version,
            models.choices.len()
        )));
        Next::Done(
            Launcher::Ready {
                start: cli.start,
                own,
                offer: Offer {
                    listed: models,
                    named: cli.defaults.named,
                    chosen: cli.defaults.chosen.map(ModelId),
                },
            },
            notes,
        )
    }
}

fn unavailable<P, S>(reason: String) -> Next<P, S> {
    let note = Note::warn(format!("unavailable: {reason}"));
    Next::Done(Launcher::Unavailable(reason), Vec::from([note]))
}

#[cfg(test)]
mod tests {
    use agents::Note;
    use proto::{Effort, ModelChoice, ModelId, Models};

    use super::{Defaults, Next, Output, signed_in, start};
    use crate::registry::{Launcher, Offer, OwnSignIn};

    type Step = Next<&'static str, &'static str>;

    fn defaults() -> Defaults {
        Defaults {
            named: Some(ModelId(String::from("named"))),
            chosen: Some(String::from("haiku")),
        }
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
        models: Result<Models, String>,
    }

    fn probes() -> Probes {
        Probes {
            installed: Ok("/pinned"),
            version: Ok(ran(0)),
            check: Ok(Some(0)),
            models: Ok(listed()),
        }
    }

    /// Walks a startup through every step.
    fn walk(
        given: Option<&'static str>,
        probes: &Probes,
    ) -> (Vec<&'static str>, Launcher<&'static str>, Vec<Note>) {
        let mut seen = Vec::new();
        let mut next: Step = start("claude", given, defaults());
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
                    listing.listed(probes.models.clone())
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
                Vec::from([Note::info(String::from(
                    "ready: /given 1.2.3 (Claude Code), signed in Unmanaged, 1 models"
                ))]),
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
    fn a_cli_that_lists_nothing_offers_only_the_defaults() {
        let (_, launcher, notes) = walk(
            Some("/given"),
            &Probes {
                models: Err(String::from("not signed in")),
                ..probes()
            },
        );
        assert_eq!(
            (launcher, notes.first()),
            (
                ready(OwnSignIn::Unmanaged, Models::none()),
                Some(&Note::warn(String::from(
                    "the CLI listed no models: not signed in"
                )))
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
            (seen.len(), launcher, notes.len())
        };
        let reason = |text: &str| Launcher::Unavailable(String::from(text));
        assert_eq!(
            [
                unavailable(Err(String::from("offline")), Ok(ran(0))),
                unavailable(Ok("/pinned"), Ok(ran(2))),
                unavailable(Ok("/pinned"), Err(String::from("no such file"))),
            ],
            [
                (1, reason("installing the pinned claude: offline"), 1),
                (2, reason("/pinned --version exited with Some(2)"), 1),
                (2, reason("/pinned does not run: no such file"), 1),
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
