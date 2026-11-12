//! The model provider in a private home's `config.toml` (§6.3). The home's
//! `model_provider`, `model` and `[model_providers.conductor]` are
//! Conductor's; everything else Codex wrote there stays.

use std::io::ErrorKind;
use std::path::Path;

use proto::{CodexCredential, WireApi};
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
    let providers = config
        .entry("model_providers")
        .or_insert(Item::Table(implicit_table()))
        .as_table_mut();
    match (credential, providers) {
        (
            Some(CodexCredential::Provider {
                name,
                base_url,
                wire_api,
                key: _,
                model,
            }),
            Some(providers),
        ) => {
            let mut provider = Table::new();
            provider.insert("name", value(name.as_str()));
            provider.insert("base_url", value(base_url.as_str()));
            provider.insert("env_key", value(KEY_VAR));
            provider.insert(
                "wire_api",
                value(match wire_api {
                    WireApi::Responses => "responses",
                }),
            );
            providers.insert(PROVIDER_ID, Item::Table(provider));
            config.insert("model_provider", value(PROVIDER_ID));
            config.insert("model", value(model.as_str()));
        }
        (None, Some(providers)) => {
            providers.remove(PROVIDER_ID);
        }
        // `model_providers` that isn't a table is Codex's to refuse.
        (_, None) => {}
    }
    Ok(config.to_string())
}

/// A table with no header of its own, so `[model_providers.conductor]` is
/// written without a bare `[model_providers]` above it.
fn implicit_table() -> Table {
    let mut table = Table::new();
    table.set_implicit(true);
    table
}

#[cfg(test)]
mod tests {
    use proto::{CodexCredential, WireApi};

    use super::with_provider;

    const CODEX_WROTE: &str =
        "check_for_update_on_startup = false\n\n[projects.\"/work\"]\ntrust_level = \"trusted\"\n";

    #[test]
    fn the_provider_goes_in_and_out_and_codex_keeps_its_own() {
        let credential = CodexCredential::Provider {
            name: String::from("Gateway"),
            base_url: String::from("https://gateway.example/v1"),
            wire_api: WireApi::Responses,
            key: String::from("sk-never-written"),
            model: String::from("gpt-6"),
        };
        let set = with_provider(CODEX_WROTE, Some(&credential)).unwrap();
        assert_eq!(
            set,
            "check_for_update_on_startup = false\nmodel_provider = \"conductor\"\nmodel = \"gpt-6\"\n\n[projects.\"/work\"]\ntrust_level = \"trusted\"\n\n[model_providers.conductor]\nname = \"Gateway\"\nbase_url = \"https://gateway.example/v1\"\nenv_key = \"CONDUCTOR_CODEX_KEY\"\nwire_api = \"responses\"\n"
        );
        assert_eq!(with_provider(&set, None).unwrap(), CODEX_WROTE);
    }
}
