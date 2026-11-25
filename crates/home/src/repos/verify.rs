//! Whether each project's default branch head has a recorded `commit`
//! pass of every profile it declares, as `home repos --verify` reports it.
//!
//! The shell runs `<profile>/check.sh commit --verify` from the checkout of
//! the project that provides the profile.

use alloc::collections::BTreeMap;
use core::fmt;

use super::Placed;
use crate::prelude::*;
use vcs::status::Checkout;

/// One `--verify` to run: a profile's, on a project's checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verify {
    pub project: String,
    pub profile: String,
    /// The head's commit, shortened.
    pub commit: String,
    pub checkout: String,
    /// The checkout of the project providing the profile, whose
    /// `<profile>/check.sh` runs.
    pub standards: String,
}

/// A `--verify` to run, or why a project's passes go unchecked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verifying {
    Run(Verify),
    Unchecked(Reason),
}

/// What a `--verify` printed, and whether it passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    pub passed: bool,
    pub out: String,
    pub err: String,
}

/// What `home repos --verify` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// No checkout of the project is level with origin's default branch.
    NoLevelCheckout {
        project: String,
        default: Option<String>,
    },
    /// No project provides the profile.
    NobodyProvides { project: String, profile: String },
    /// The project providing the profile has no checkout here.
    ProviderAbsent {
        project: String,
        profile: String,
        provider: String,
    },
    /// No checkout of a provider is level with origin's default branch, so
    /// its check.sh would judge by standards origin doesn't hold.
    ProviderOff {
        provider: String,
        path: String,
        default: Option<String>,
    },
    /// The head lacks a recorded pass of these units.
    Missing { verify: Verify, units: Vec<String> },
    /// check.sh failed other than for a missing pass.
    CheckFailed {
        project: String,
        profile: String,
        why: String,
    },
}

impl fmt::Display for Reason {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        let at = |default: &Option<String>| {
            default.as_ref().map_or_else(
                || String::from("origin's default branch"),
                |branch| format!("origin/{branch}"),
            )
        };
        match self {
            Self::NoLevelCheckout { project, default } => writeln!(
                out,
                "{project}: no checkout clean at {}, so its recorded passes \
                 go unchecked",
                at(default)
            ),
            Self::NobodyProvides { project, profile } => writeln!(
                out,
                "{project}: nobody provides {profile}, so its recorded \
                 passes go unchecked"
            ),
            Self::ProviderAbsent {
                project,
                profile,
                provider,
            } => writeln!(
                out,
                "{project}: {provider} isn't checked out here, so its \
                 {profile} passes go unchecked"
            ),
            Self::ProviderOff {
                provider,
                path,
                default,
            } => writeln!(
                out,
                "{provider} at {path} isn't clean at {}; fix: bring it level \
                 with origin",
                at(default)
            ),
            Self::Missing { verify, units } => writeln!(
                out,
                "{}: no recorded `commit` pass for {} at {} ({}); fix: run \
                 `{}/check.sh commit {}` from {}",
                verify.project,
                verify.profile,
                verify.commit,
                units.join(", "),
                verify.profile,
                verify.checkout,
                verify.standards
            ),
            Self::CheckFailed {
                project,
                profile,
                why,
            } => writeln!(out, "{project}: {profile}/check.sh failed: {why}"),
        }
    }
}

/// The checkouts whose origin default branch [`verifying`] needs: those of
/// projects that declare or provide profiles.
#[must_use]
pub fn asks(placed: &[Placed]) -> Vec<String> {
    placed
        .iter()
        .filter(|project| {
            !project.profiles.is_empty() || !project.provides.is_empty()
        })
        .flat_map(|project| project.checkouts.iter().cloned())
        .collect()
}

enum Provider<'a> {
    At(&'a str),
    Not(Reason),
}

/// The `--verify`s `home repos --verify` runs, or why each project's passes
/// go unchecked.
///
/// It runs one per profile of each project with profiles and a checkout
/// here, on its first checkout level with origin's default branch, from the
/// first such checkout of the project providing the profile. `--verify`
/// keys the working tree, so a checkout off that head or with changes says
/// nothing of the head, and a provider's checkout off it judges by other
/// standards: while one is, only it is reported. `states` and `defaults`
/// are by path from the home at `home`, as [`super::placed`] gives them;
/// `defaults` holds origin's default branch of the checkouts [`asks`]
/// gives.
#[must_use]
pub fn verifying(
    home: &str,
    placed: &[Placed],
    states: &BTreeMap<String, Option<Checkout>>,
    defaults: &BTreeMap<String, String>,
) -> Vec<Verifying> {
    let verified: Vec<&Placed> = placed
        .iter()
        .filter(|project| {
            !project.profiles.is_empty() && !project.checkouts.is_empty()
        })
        .collect();
    let off: Vec<Verifying> = providers(placed, &verified)
        .filter(|provider| level(provider, states, defaults).is_none())
        .map(|provider| {
            Verifying::Unchecked(Reason::ProviderOff {
                provider: provider.name.clone(),
                path: joined(
                    home,
                    provider
                        .checkouts
                        .first()
                        .map_or(super::HOME, String::as_str),
                ),
                default: default_of(provider, defaults),
            })
        })
        .collect();
    if !off.is_empty() {
        return off;
    }
    verified
        .into_iter()
        .flat_map(|project| {
            let name = &project.name;
            let Some((path, commit)) = level(project, states, defaults) else {
                return Vec::from([Verifying::Unchecked(
                    Reason::NoLevelCheckout {
                        project: name.clone(),
                        default: default_of(project, defaults),
                    },
                )]);
            };
            project
                .profiles
                .iter()
                .map(|profile| {
                    match provider(placed, name, profile, states, defaults) {
                        Provider::Not(reason) => Verifying::Unchecked(reason),
                        Provider::At(standards) => Verifying::Run(Verify {
                            project: name.clone(),
                            profile: profile.clone(),
                            commit: String::from(
                                commit.get(..7).unwrap_or(commit),
                            ),
                            checkout: joined(home, path),
                            standards: joined(home, standards),
                        }),
                    }
                })
                .collect()
        })
        .collect()
}

fn level<'a>(
    project: &'a Placed,
    states: &'a BTreeMap<String, Option<Checkout>>,
    defaults: &BTreeMap<String, String>,
) -> Option<(&'a str, &'a str)> {
    project.checkouts.iter().find_map(|path| {
        let state = states.get(path)?.as_ref()?;
        state
            .is_level(defaults.get(path)?)
            .then_some((path.as_str(), state.commit.as_deref()?))
    })
}

fn default_of(
    project: &Placed,
    defaults: &BTreeMap<String, String>,
) -> Option<String> {
    project
        .checkouts
        .iter()
        .find_map(|path| defaults.get(path))
        .cloned()
}

fn providers<'a>(
    placed: &'a [Placed],
    verified: &[&Placed],
) -> impl Iterator<Item = &'a Placed> {
    let declared: Vec<String> = verified
        .iter()
        .flat_map(|project| project.profiles.iter().cloned())
        .collect();
    placed.iter().filter(move |project| {
        !project.checkouts.is_empty()
            && project.provides.iter().any(|name| declared.contains(name))
    })
}

fn provider<'a>(
    placed: &'a [Placed],
    project: &str,
    profile: &str,
    states: &'a BTreeMap<String, Option<Checkout>>,
    defaults: &BTreeMap<String, String>,
) -> Provider<'a> {
    let mut providing = placed
        .iter()
        .filter(|candidate| {
            candidate.provides.iter().any(|name| name == profile)
        })
        .peekable();
    let Some(first) = providing.peek().copied() else {
        return Provider::Not(Reason::NobodyProvides {
            project: String::from(project),
            profile: String::from(profile),
        });
    };
    providing
        .find_map(|candidate| level(candidate, states, defaults))
        .map_or_else(
            || {
                Provider::Not(Reason::ProviderAbsent {
                    project: String::from(project),
                    profile: String::from(profile),
                    provider: first.name.clone(),
                })
            },
            |(path, _)| Provider::At(path),
        )
}

fn joined(home: &str, path: &str) -> String {
    let top = home.trim_end_matches(['/', '\\']);
    let anywhere = path.starts_with(['/', '\\']) || path.get(1..2) == Some(":");
    match path {
        super::HOME => String::from(top),
        _ if anywhere => String::from(path),
        _ => format!("{top}/{path}"),
    }
}

fn failing(line: &str) -> Option<(&str, &str)> {
    let mut words = line.split_whitespace();
    let unit = words.next()?;
    let rest = line.trim_start().strip_prefix(unit)?.trim_start();
    Some((unit, rest.strip_prefix("FAIL")?.trim_start()))
}

const MISSING_PASS_PREFIX: &str = "no recorded pass of ";

/// What `home repos --verify` says of `verify` from how it `ran`.
///
/// Nothing when it passed; the units without a recorded pass when those
/// are all that failed; else that check.sh failed, by its first line on
/// stderr or, without one, its first failing line.
#[must_use]
pub fn verdict(verify: &Verify, ran: &Ran) -> Option<Reason> {
    if ran.passed {
        return None;
    }
    let failed: Vec<(&str, &str)> =
        ran.out.lines().filter_map(failing).collect();
    if !failed.is_empty()
        && failed
            .iter()
            .all(|(_, why)| why.starts_with(MISSING_PASS_PREFIX))
    {
        let units = failed.iter().fold(Vec::new(), |mut units, (unit, _)| {
            if !units.iter().any(|named: &String| named == unit) {
                units.push(String::from(*unit));
            }
            units
        });
        return Some(Reason::Missing {
            verify: verify.clone(),
            units,
        });
    }
    let first_failing = || {
        ran.out.lines().find(|line| {
            failing(line)
                .is_some_and(|(_, why)| !why.starts_with(MISSING_PASS_PREFIX))
        })
    };
    let why = ran
        .err
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .or_else(first_failing)
        .unwrap_or("it exited without saying why");
    Some(Reason::CheckFailed {
        project: verify.project.clone(),
        profile: verify.profile.clone(),
        why: String::from(why.trim_end()),
    })
}

#[cfg(test)]
mod tests;
