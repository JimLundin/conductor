//! Whether each project's default branch head has a recorded `commit`
//! pass of every profile it declares, as `home repos` reports it.
//!
//! The shell runs `<profile>/check.sh commit --verify` from the checkout of the
//! project that provides the profile.

use alloc::collections::BTreeMap;

use super::Placed;
use crate::prelude::*;

/// What Git printed of a checkout's head.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Head {
    /// `git rev-parse --short HEAD`.
    pub commit: Option<String>,
    /// `git symbolic-ref --short refs/remotes/origin/HEAD`, as
    /// `origin/<branch>`.
    pub default: Option<String>,
}

/// One `--verify` to run: a profile's, on a project's checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verify {
    pub project: String,
    pub profile: String,
    pub commit: String,
    pub checkout: String,
    /// The checkout of the project providing the profile, whose
    /// `<profile>/check.sh` runs.
    pub standards: String,
}

/// A `--verify` to run, or why one isn't.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verifying {
    Run(Verify),
    Said(String),
}

/// What a `--verify` printed, and whether it passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    pub passed: bool,
    pub out: String,
}

/// The `--verify`s `home repos` runs.
///
/// It runs one per profile of each project with profiles and a checkout
/// here, on its first checkout that is clean at its default branch's head,
/// from the first checkout of the project providing the profile. `--verify`
/// keys the working tree, so a checkout
/// off that head or with changes says nothing of the head. Paths are from
/// the home at `home`, as [`super::placed`] gives them.
#[must_use]
pub fn verifying(
    home: &str,
    placed: &[Placed],
    states: &BTreeMap<String, Option<String>>,
    heads: &BTreeMap<String, Head>,
) -> Vec<Verifying> {
    placed
        .iter()
        .filter(|project| {
            !project.profiles.is_empty() && !project.checkouts.is_empty()
        })
        .flat_map(|project| {
            let name = &project.name;
            let at_head = project.checkouts.iter().find_map(|path| {
                at_head(states.get(path)?.as_deref()?, heads.get(path)?)
                    .map(|commit| (path, commit))
            });
            let Some((path, commit)) = at_head else {
                let default = project
                    .checkouts
                    .iter()
                    .find_map(|path| heads.get(path)?.default.as_deref())
                    .map_or("its default branch", str::trim_end);
                return Vec::from([Verifying::Said(format!(
                    "{name}: no checkout clean at {default}, so its \
                     recorded passes go unchecked\n"
                ))]);
            };
            project
                .profiles
                .iter()
                .map(|profile| {
                    provider(placed, profile).map_or_else(
                        || {
                            Verifying::Said(format!(
                                "{name}: no project checked out here provides \
                                 {profile}, so its recorded passes go \
                                 unchecked\n"
                            ))
                        },
                        |standards| {
                            Verifying::Run(Verify {
                                project: name.clone(),
                                profile: profile.clone(),
                                commit: commit.clone(),
                                checkout: joined(home, path),
                                standards: joined(home, standards),
                            })
                        },
                    )
                })
                .collect()
        })
        .collect()
}

/// The commit a checkout is at, if it is on its default branch, level with
/// origin's, with nothing changed or untracked, from what
/// `git status --porcelain --branch` printed in it.
fn at_head(porcelain: &str, head: &Head) -> Option<String> {
    let default = head.default.as_deref()?.trim_end();
    let (_, branch) = default.split_once('/')?;
    let mut lines = porcelain.lines().filter(|line| !line.is_empty());
    let level = lines.next() == Some(&format!("## {branch}...{default}"));
    (level && lines.next().is_none())
        .then(|| head.commit.as_deref().map(str::trim_end).map(String::from))
        .flatten()
}

/// The first checkout of the project providing `profile`.
fn provider<'a>(placed: &'a [Placed], profile: &str) -> Option<&'a String> {
    placed
        .iter()
        .filter(|project| project.provides.iter().any(|name| name == profile))
        .find_map(|project| project.checkouts.first())
}

/// A path from the home at `home` as a path anywhere.
fn joined(home: &str, path: &str) -> String {
    let top = home.trim_end_matches(['/', '\\']);
    let anywhere = path.starts_with(['/', '\\']) || path.get(1..2) == Some(":");
    match path {
        super::HOME => String::from(top),
        _ if anywhere => String::from(path),
        _ => format!("{top}/{path}"),
    }
}

/// What `home repos` says of `verify` from how it `ran`, `None` where it
/// couldn't run: nothing when it passed, else the units without a recorded
/// pass and the command that runs them.
#[must_use]
pub fn verdict(verify: &Verify, ran: Option<&Ran>) -> String {
    let Verify {
        project,
        profile,
        commit,
        checkout,
        standards,
    } = verify;
    let units = ran.map_or_else(Vec::new, |printed| {
        printed.out.lines().fold(Vec::new(), |mut units, line| {
            let mut words = line.split_whitespace();
            let unit = words.next().unwrap_or_default();
            if words.next() == Some("FAIL") && !units.contains(&unit) {
                units.push(unit);
            }
            units
        })
    });
    match (ran.is_some_and(|printed| printed.passed), units.is_empty()) {
        (true, _) => String::new(),
        (false, true) => format!(
            "{project}: verifying {profile} at {commit} failed without \
             naming a unit; see `{profile}/check.sh commit --verify \
             {checkout}` from {standards}\n"
        ),
        (false, false) => format!(
            "{project}: no recorded `commit` pass for {profile} at {commit} \
             ({}); fix: run `{profile}/check.sh commit {checkout}` from \
             {standards}\n",
            units.join(", ")
        ),
    }
}

#[cfg(test)]
mod tests;
