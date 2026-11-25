//! What `home_files::install` decides.
//!
//! Where the home's skills are, where each harness reads them, and which to
//! remove and copy, in place of the copies made before and beside skills
//! that came from elsewhere.

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

/// What `home_files::install` does to the skills folder `folder`, given
/// its listing's text if it has one, the names in it, and each folder of
/// the home's [`SKILLS`] with whether it holds a [`SKILL_FILE`].
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

fn is_name(name: &str) -> bool {
    !name.is_empty()
        && !matches!(name, "." | "..")
        && !name.contains(['/', '\\'])
}

/// Whether a host installs its clone's skills now, for the user it has,
/// if any: once per commit the clone moves to, never before it has one.
/// The user and the commit to record, when it does.
#[must_use]
pub fn due<T>(
    user: Option<T>,
    head: Option<String>,
    installed: Option<&str>,
) -> Option<(T, String)> {
    let moved = head.filter(|commit| Some(commit.as_str()) != installed)?;
    user.map(|to| (to, moved))
}
