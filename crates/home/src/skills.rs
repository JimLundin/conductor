use crate::prelude::*;

pub const SKILLS: &str = "skills";

pub const SKILL_FILE: &str = "SKILL.md";

pub const HARNESSES: [&str; 2] = [".claude/skills", ".agents/skills"];

pub const LISTING: &str = ".home-skills";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub remove: Vec<String>,
    pub copy: Vec<String>,
    pub listing: String,
    pub said: String,
}

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

#[must_use]
pub fn due<T>(
    user: Option<T>,
    head: Option<String>,
    installed: Option<&str>,
) -> Option<(T, String)> {
    let moved = head.filter(|commit| Some(commit.as_str()) != installed)?;
    user.map(|to| (to, moved))
}
