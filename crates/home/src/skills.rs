//! `home skills`: the home's `skills/` copied into each harness's skills
//! folder in the user's home, in place of the copies made before, beside
//! skills that came from elsewhere.

use crate::prelude::*;

/// The folder of the home whose folders are skills.
pub const SKILLS: &str = "skills";

/// The file in a folder of [`SKILLS`] that makes it a skill.
pub const SKILL_FILE: &str = "SKILL.md";

/// Each harness's skills folder, from the user's home: Claude Code reads
/// `.claude/skills`, Codex and Gemini `.agents/skills`.
pub const HARNESSES: [&str; 2] = [".claude/skills", ".agents/skills"];

/// The file in a harness's skills folder that lists the skills copied
/// there from the home, one per line, so the next copy removes only those.
pub const LISTING: &str = ".home-skills";

/// What to do to one harness's skills folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The folders copied before, to remove first.
    pub remove: Vec<String>,
    /// The home's skills to copy in.
    pub copy: Vec<String>,
    /// The listing's new text.
    pub listing: String,
    pub said: String,
}

/// The plan for the skills folder `folder`, given its listing's text if it
/// has one, the names in it, and each folder of the home's [`SKILLS`] with
/// whether it holds a [`SKILL_FILE`].
#[must_use]
pub fn plan(
    folder: &str,
    listing: Option<&str>,
    present: &[String],
    mut offered: Vec<(String, bool)>,
) -> Plan {
    let remove: Vec<String> = listing
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|name| is_name(name))
        .map(String::from)
        .collect();
    offered.sort();
    let (copy, skipped): (Vec<String>, Vec<String>) = offered
        .into_iter()
        .filter(|(_, skill)| *skill)
        .map(|(name, _)| name)
        .partition(|name| !present.contains(name) || remove.contains(name));
    let said = skipped
        .iter()
        .map(|name| {
            format!(
                "skip {name}: {folder}/{name} was not copied from the home\n"
            )
        })
        .chain(core::iter::once(format!(
            "{folder}: {} skills from the home\n",
            copy.len()
        )))
        .collect();
    Plan {
        listing: copy
            .iter()
            .map(|name| format!("{name}\n"))
            .collect::<Vec<String>>()
            .concat(),
        remove,
        copy,
        said,
    }
}

/// A listed name is a folder in the skills folder, never a path out of it.
fn is_name(name: &str) -> bool {
    !name.is_empty()
        && !matches!(name, "." | "..")
        && !name.contains(['/', '\\'])
}

#[cfg(test)]
mod tests;
