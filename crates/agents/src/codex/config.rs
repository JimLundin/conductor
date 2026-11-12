//! The model provider in a private home's `config.toml` (§6.3). The home's
//! `model_provider`, `model` and `[model_providers.conductor]` are
//! Conductor's; everything else Codex wrote there stays.

use std::io::ErrorKind;
use std::path::Path;

use proto::CodexCredential;
use toml_edit::{DocumentMut, Item, Table, value};

use super::server::Error;

/// Our provider's id in `model_providers`.
const PROVIDER_ID: &str = "conductor";
/// The variable our provider's `env_key` names, which carries its key.
pub(crate) const KEY_VAR: &str = "CONDUCTOR_CODEX_KEY";

/// Writes `credential`'s provider into `<home>/config.toml`, or takes ours
/// out for `None`. The key stays out of the file.
pub(crate) fn write(home: &Path, credential: Option<&CodexCredential>) -> Result<(), Error> {
    let path = home.join("config.toml");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => Ok(text),
        Err(err) => match err.kind() {
            ErrorKind::NotFound => Ok(String::new()),
            _ => Err(err),
        },
    }?;
    let config = with_provider(&text, credential)
        .map_err(|err| Error::Command(format!("{}: {err}", path.display())))?;
    Ok(std::fs::write(&path, config)?)
}

fn with_provider(
    text: &str,
    credential: Option<&CodexCredential>,
) -> Result<String, toml_edit::TomlError> {
    let mut config = text.parse::<DocumentMut>()?;
    config.remove("model_provider");
    config.remove("model");
    let root = config.as_table_mut();
    // A section that isn't a table is Codex's to refuse.
    match credential {
        Some(CodexCredential::Provider {
            name,
            base_url,
            key: _,
            model,
        }) => {
            let mut provider = Table::new();
            provider.insert("name", value(name.as_str()));
            provider.insert("base_url", value(base_url.as_str()));
            provider.insert("env_key", value(KEY_VAR));
            provider.insert("wire_api", value("responses"));
            if let Some(providers) = section(root, String::from("model_providers")) {
                providers.insert(PROVIDER_ID, Item::Table(provider));
            }
            if let Some(filters) = shell_filters(root) {
                filters.insert(KEY_VAR, value("exclude"));
            }
            root.insert("model_provider", value(PROVIDER_ID));
            root.insert("model", value(model.as_str()));
        }
        None => {
            if let Some(providers) = section(root, String::from("model_providers")) {
                providers.remove(PROVIDER_ID);
            }
            if let Some(policy) = section(root, String::from("shell_environment_policy")) {
                if let Some(filters) = section(policy, String::from("filters")) {
                    filters.remove(KEY_VAR);
                }
                // Ours was its only filter: a bare header would be left.
                policy.retain(|key, item| {
                    key != "filters" || !item.as_table().is_some_and(Table::is_empty)
                });
            }
        }
    }
    Ok(config.to_string())
}

/// `shell_environment_policy.filters`, where our exclude keeps the key out
/// of every command Codex runs. Codex 0.166 passes variables named like a
/// key to them by default (`ignore_default_excludes` defaults to true in
/// `config/src/shell_environment_policy.rs`).
fn shell_filters(root: &mut Table) -> Option<&mut Table> {
    section(root, String::from("shell_environment_policy"))
        .and_then(|policy| section(policy, String::from("filters")))
}

/// The table at `key`, created with no header of its own, so
/// `[model_providers.conductor]` is written without a bare
/// `[model_providers]` above it.
fn section(table: &mut Table, key: String) -> Option<&mut Table> {
    let mut implicit = Table::new();
    implicit.set_implicit(true);
    table
        .entry(&key)
        .or_insert(Item::Table(implicit))
        .as_table_mut()
}

#[cfg(test)]
mod tests {
    use proto::CodexCredential;

    use super::with_provider;

    const CODEX_WROTE: &str =
        "check_for_update_on_startup = false\n\n[projects.\"/work\"]\ntrust_level = \"trusted\"\n";

    #[test]
    fn the_provider_goes_in_and_out_and_codex_keeps_its_own() {
        let credential = CodexCredential::Provider {
            name: String::from("Gateway"),
            base_url: String::from("https://gateway.example/v1"),
            key: String::from("sk-never-written"),
            model: String::from("gpt-6"),
        };
        let set = with_provider(CODEX_WROTE, Some(&credential)).unwrap();
        assert_eq!(
            set,
            "check_for_update_on_startup = false\nmodel_provider = \"conductor\"\nmodel = \"gpt-6\"\n\n[projects.\"/work\"]\ntrust_level = \"trusted\"\n\n[model_providers.conductor]\nname = \"Gateway\"\nbase_url = \"https://gateway.example/v1\"\nenv_key = \"CONDUCTOR_CODEX_KEY\"\nwire_api = \"responses\"\n\n[shell_environment_policy.filters]\nCONDUCTOR_CODEX_KEY = \"exclude\"\n"
        );
        assert_eq!(with_provider(&set, None).unwrap(), CODEX_WROTE);
    }
}
