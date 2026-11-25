use install::Home;
use std::fs::File;
use std::io::{self, ErrorKind, Read, Seek, Write};
use std::path::Path;

use codex::CodexCredential;
use codex::Edit;
use toml_edit::{DocumentMut, Item, Table, TableLike, value};

use super::server::Error;

pub fn write(
    home: &Home,
    credential: Option<&CodexCredential>,
) -> Result<(), Error> {
    let Home::Private(dir) = home else {
        return Ok(());
    };
    let path = dir.join("config.toml");
    let failed = |err| Error::Command(format!("{}: {err}", path.display()));
    let text = read_or_empty(&path).map_err(failed)?;
    let config = with_provider(&text, credential).map_err(|reason| {
        Error::Command(format!("{}: {reason}", path.display()))
    })?;
    replace(&path, |file| file.write_all(config.as_bytes())).map_err(failed)
}

fn replace(
    path: &Path,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("missing config directory"))?;
    let mut next = tempfile::NamedTempFile::new_in(dir)?;
    write(next.as_file_mut())?;
    next.rewind()?;
    let mut staged = String::new();
    next.read_to_string(&mut staged)?;
    staged.parse::<DocumentMut>().map_err(|_| {
        io::Error::new(ErrorKind::InvalidData, "invalid replacement TOML")
    })?;
    match path.metadata() {
        Ok(metadata) => {
            next.as_file().set_permissions(metadata.permissions())?;
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    next.as_file().sync_all()?;
    next.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn read_or_empty(path: &Path) -> std::io::Result<String> {
    match std::fs::read_to_string(path) {
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(String::new()),
        read => read,
    }
}

fn with_provider(
    text: &str,
    credential: Option<&CodexCredential>,
) -> Result<String, String> {
    let mut config = text
        .parse::<DocumentMut>()
        .map_err(|_| String::from("invalid TOML"))?;
    for edit in codex::config_edits(credential) {
        apply(config.as_table_mut(), edit)?;
    }
    Ok(config.to_string())
}

fn apply(root: &mut Table, edit: Edit) -> Result<(), String> {
    match edit {
        Edit::Remove(path) => {
            let _ = edit_nested_key(root, &path, false, |table, key| {
                table.remove(key);
            });
            Ok(())
        }
        Edit::Set { path, value: text } => {
            edit_nested_key(root, &path, true, |table, key| {
                table.insert(key, value(text));
            })
        }
        Edit::Table { path, entries } => {
            edit_nested_key(root, &path, true, |table, key| {
                table.insert(key, Item::Table(entries.into_iter().collect()));
            })
        }
        Edit::RemoveIfEmpty(path) => {
            let _ = edit_nested_key(root, &path, false, |table, key| {
                if table
                    .get(key)
                    .and_then(Item::as_table_like)
                    .is_some_and(TableLike::is_empty)
                {
                    table.remove(key);
                }
            });
            Ok(())
        }
    }
}

fn edit_nested_key(
    root: &mut dyn TableLike,
    path: &[String],
    create: bool,
    change: impl FnOnce(&mut dyn TableLike, &str),
) -> Result<(), String> {
    let (key, tables) = path.split_last().ok_or("empty config key")?;
    let table = tables.iter().try_fold(root, |table, name| {
        if create && !table.contains_key(name) {
            let mut implicit = Table::new();
            implicit.set_implicit(true);
            table.insert(name, Item::Table(implicit));
        }
        table
            .get_mut(name)
            .and_then(Item::as_table_like_mut)
            .ok_or_else(|| format!("{name} must be a table"))
    })?;
    change(table, key);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};

    use codex::CodexCredential;
    use toml_edit::DocumentMut;

    use super::{replace, with_provider};

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
    fn an_openai_key_replaces_the_gateway_endpoint_without_writing_the_key()
    -> Result<(), Box<dyn std::error::Error>> {
        let gateway = CodexCredential::Provider {
            name: String::from("Gateway"),
            base_url: String::from("https://gateway.example/v1"),
            key: String::from("synthetic-gateway-key"),
            model: String::from("gateway-model"),
        };
        let openai = CodexCredential::ApiKey {
            key: String::from("synthetic-api-key"),
        };
        let before = with_provider(CODEX_WROTE, Some(&gateway))?;
        let direct = with_provider(&before, Some(&openai))?;
        let doc = direct.parse::<DocumentMut>()?;
        let provider = &doc["model_providers"]["conductor"];
        assert_eq!(
            provider["base_url"].as_str(),
            Some("https://api.openai.com/v1")
        );
        assert_eq!(provider["env_key"].as_str(), Some("CONDUCTOR_CODEX_KEY"));
        assert_eq!(doc["model_provider"].as_str(), Some("conductor"));
        assert!(!doc.contains_key("model"));
        assert_eq!(
            doc["shell_environment_policy"]["filters"]["CONDUCTOR_CODEX_KEY"]
                .as_str(),
            Some("exclude"),
        );
        assert!(!direct.contains("synthetic-api-key"));
        let back =
            with_provider(&direct, Some(&gateway))?.parse::<DocumentMut>()?;
        assert_eq!(
            back["model_providers"]["conductor"]["base_url"].as_str(),
            Some("https://gateway.example/v1")
        );
        assert_eq!(back["model"].as_str(), Some("gateway-model"));
        assert_eq!(with_provider(&direct, None)?, CODEX_WROTE);
        Ok(())
    }

    #[test]
    fn inline_and_ordinary_sections_preserve_other_settings()
    -> Result<(), Box<dyn std::error::Error>> {
        let inline = "model_providers = { other = { name = 'Other' } }\nshell_environment_policy = { inherit = 'all', filters = { OTHER = 'include' } }\n";
        let ordinary = "[model_providers.other]\nname = 'Other'\n[shell_environment_policy]\ninherit = 'all'\n[shell_environment_policy.filters]\nOTHER = 'include'\n";
        let credential = CodexCredential::Provider {
            name: String::from("Gateway"),
            base_url: String::from("https://gateway.example/v1"),
            key: String::from("sk-never-written"),
            model: String::from("gpt-6"),
        };
        for source in [inline, ordinary] {
            let configured = with_provider(source, Some(&credential))?;
            let doc = configured.parse::<DocumentMut>()?;
            assert_eq!(
                doc["model_providers"]["conductor"]["name"].as_str(),
                Some("Gateway")
            );
            assert_eq!(
                doc["model_providers"]["other"]["name"].as_str(),
                Some("Other")
            );
            let policy = &doc["shell_environment_policy"];
            assert_eq!(policy["inherit"].as_str(), Some("all"));
            assert_eq!(
                policy["filters"]["CONDUCTOR_CODEX_KEY"].as_str(),
                Some("exclude")
            );
            assert_eq!(policy["filters"]["OTHER"].as_str(), Some("include"));

            let cleaned =
                with_provider(&configured, None)?.parse::<DocumentMut>()?;
            assert!(
                cleaned["model_providers"]
                    .as_table_like()
                    .and_then(|providers| providers.get("conductor"))
                    .is_none()
            );
            assert_eq!(
                cleaned["model_providers"]["other"]["name"].as_str(),
                Some("Other")
            );
            assert_eq!(
                cleaned["shell_environment_policy"]["filters"]["OTHER"]
                    .as_str(),
                Some("include")
            );
        }
        Ok(())
    }

    #[test]
    fn a_failed_or_invalid_replacement_preserves_the_file()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
        let path = dir.path().join("config.toml");
        std::fs::write(&path, CODEX_WROTE)?;
        let failed = replace(&path, |file| {
            file.write_all(b"partial")?;
            Err(io::Error::other("test write failure"))
        });
        assert!(failed.is_err());
        assert_eq!(std::fs::read_to_string(&path)?, CODEX_WROTE);

        let invalid = replace(&path, |file| file.write_all(b"invalid"));
        assert_eq!(
            invalid.err().map(|err| err.kind()),
            Some(io::ErrorKind::InvalidData)
        );
        assert_eq!(std::fs::read_to_string(&path)?, CODEX_WROTE);
        assert_eq!(std::fs::read_dir(dir.path())?.count(), 1);
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
