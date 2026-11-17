//! The model provider in a private home's `config.toml` (§6.3): applies
//! the edits `codex::config_edits` decides, keeping what Codex wrote there.

use std::io::ErrorKind;
use std::path::Path;

use codex::Edit;
use proto::CodexCredential;
use toml_edit::{DocumentMut, Item, Table, value};

use super::server::Error;

/// Writes `credential`'s provider into `<home>/config.toml`, or takes ours
/// out for `None`.
pub fn write(
    home: &Path,
    credential: Option<&CodexCredential>,
) -> Result<(), Error> {
    let path = home.join("config.toml");
    let text = read(&path)?;
    let config = with_provider(&text, credential)
        .map_err(|err| Error::Command(format!("{}: {err}", path.display())))?;
    Ok(std::fs::write(&path, config)?)
}

/// The file's text; a home with none yet has an empty one.
fn read(path: &Path) -> std::io::Result<String> {
    match std::fs::read_to_string(path) {
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(String::new()),
        read => read,
    }
}

fn with_provider(
    text: &str,
    credential: Option<&CodexCredential>,
) -> Result<String, toml_edit::TomlError> {
    let mut config = text.parse::<DocumentMut>()?;
    for edit in codex::config_edits(credential) {
        apply(config.as_table_mut(), edit);
    }
    Ok(config.to_string())
}

fn apply(root: &mut Table, edit: Edit) {
    match edit {
        Edit::Remove(path) => {
            at(root, &path, |table, key| {
                table.remove(key);
            });
        }
        Edit::Set { path, value: text } => at(root, &path, |table, key| {
            table.insert(key, value(text));
        }),
        Edit::Table { path, entries } => at(root, &path, |table, key| {
            table.insert(key, Item::Table(entries.into_iter().collect()));
        }),
        Edit::RemoveIfEmpty(path) => at(root, &path, |table, key| {
            table.retain(|name, item| {
                name != key || !item.as_table().is_some_and(Table::is_empty)
            });
        }),
    }
}

/// Runs `change` on the table that holds the last key of `path`, if every
/// table on the way is one.
fn at(
    root: &mut Table,
    path: &[String],
    change: impl FnOnce(&mut Table, &str),
) {
    if let Some((key, tables)) = path.split_last()
        && let Some(table) = tables
            .iter()
            .try_fold(root, |table, name| section(table, name))
    {
        change(table, key);
    }
}

/// The table at `key`, created with no header of its own.
fn section<'a>(table: &'a mut Table, key: &str) -> Option<&'a mut Table> {
    let mut implicit = Table::new();
    implicit.set_implicit(true);
    table
        .entry(key)
        .or_insert(Item::Table(implicit))
        .as_table_mut()
}

#[cfg(test)]
mod tests {
    use proto::CodexCredential;

    use super::with_provider;

    const CODEX_WROTE: &str = "check_for_update_on_startup = false\n\n[projects.\"/work\"]\ntrust_level = \"trusted\"\n";

    #[test]
    fn the_provider_goes_in_and_out_and_codex_keeps_its_own()
    -> Result<(), Box<dyn std::error::Error>> {
        let credential = CodexCredential::Provider {
            name: String::from("Gateway"),
            base_url: String::from("https://gateway.example/v1"),
            key: String::from("sk-never-written"),
            model: String::from("gpt-6"),
        };
        let set = with_provider(CODEX_WROTE, Some(&credential))?;
        assert_eq!(
            set,
            "check_for_update_on_startup = false\nmodel_provider = \"conductor\"\nmodel = \"gpt-6\"\n\n[projects.\"/work\"]\ntrust_level = \"trusted\"\n\n[model_providers.conductor]\nname = \"Gateway\"\nbase_url = \"https://gateway.example/v1\"\nenv_key = \"CONDUCTOR_CODEX_KEY\"\nwire_api = \"responses\"\n\n[shell_environment_policy.filters]\nCONDUCTOR_CODEX_KEY = \"exclude\"\n"
        );
        assert_eq!(with_provider(&set, None)?, CODEX_WROTE);
        Ok(())
    }

    #[test]
    fn a_section_that_is_not_a_table_is_left_alone()
    -> Result<(), Box<dyn std::error::Error>> {
        let odd = "model_providers = 1\nshell_environment_policy = \"all\"\n";
        assert_eq!(with_provider(odd, None)?, odd);
        Ok(())
    }
}
