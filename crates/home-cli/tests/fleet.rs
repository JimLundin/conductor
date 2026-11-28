use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use fleet_store::{HOST_KEY, STORE, STORE_KEY, State, crypto};
use host::fleet::{
    Configuration, HostEntry, Identity, ProjectCredential, Role, Snapshot,
};
use iroh_base::SecretKey;

const FOLDER: &str = "7w85je";
const REFERENCE: &str = "fleet/tracker";
const ELSEWHERE: &str = "repository = 'owner/other'\n[source]\nkind = 'classic'\ntoken = 'fixture'\n";

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _removed = fs::remove_dir_all(&self.0);
    }
}

fn hex(key: &[u8; 32]) -> String {
    data_encoding::HEXLOWER.encode(key)
}

fn join_fleet(data: &Path, shared: Option<&str>) -> Result<(), Box<dyn Error>> {
    let root = SecretKey::from_bytes(&[5; 32]);
    let host = SecretKey::from_bytes(&[6; 32]);
    let fleet = *root.public().as_bytes();
    let node = *host.public().as_bytes();
    let identity = Identity {
        fleet,
        device: node,
        name: String::from("desk"),
        role: Role::Host,
    };
    let config = Configuration {
        snapshot: Snapshot {
            fleet,
            root: node,
            version: 1,
            hosts: vec![HostEntry {
                node,
                name: String::from("desk"),
                relay: String::from("https://relay.example/"),
                added_at: 1,
            }],
            revoked: Vec::new(),
        },
        instances: host::registry::Roster::default(),
        project_credentials: shared
            .map(|entry| {
                (
                    String::from(REFERENCE),
                    ProjectCredential::new(entry.to_owned()),
                )
            })
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
    };
    let state = State {
        certificate: crypto::sign_identity(&root, &identity)?,
        current: crypto::sign_config(&root, config)?,
        root_key: None,
        invites: Vec::new(),
        revoked: None,
        applied: BTreeSet::new(),
    };
    let store_key = [7; 32];
    fs::create_dir_all(data)?;
    fs::write(data.join(HOST_KEY), hex(&host.to_bytes()))?;
    fs::write(data.join(STORE_KEY), hex(&store_key))?;
    fs::write(data.join(STORE), crypto::seal(&store_key, &state)?)?;
    Ok(())
}

fn tracked(name: &str) -> Result<Scratch, Box<dyn Error>> {
    let home = Scratch(
        std::env::temp_dir()
            .join(format!("home-fleet-{name}-{}", std::process::id())),
    );
    let project = home.0.join("projects").join(FOLDER);
    fs::create_dir_all(&project)?;
    fs::create_dir_all(home.0.join("docs/agents"))?;
    fs::write(
        home.0.join("docs/agents/flow.toml"),
        include_str!("../../home/src/flow/test-flow.toml"),
    )?;
    fs::write(
        project.join("project.toml"),
        format!(
            "name = \"sample\"\ntitle = \"Sample\"\nroot = \"e22b7f7b2976650400965d4147ba771675f96076\"\n[backends.tracker]\nadapter = \"github\"\nresource = \"owner/issues\"\ncredential = \"{REFERENCE}\"\n"
        ),
    )?;
    Ok(home)
}

fn home(
    scratch: &Scratch,
    args: &[&str],
) -> Result<(String, bool), Box<dyn Error>> {
    let ran = Command::new(env!("CARGO_BIN_EXE_home"))
        .current_dir(scratch.0.join("projects").join(FOLDER))
        .env("HOME", &scratch.0)
        .env("XDG_DATA_HOME", scratch.0.join("share"))
        .args(args)
        .output()?;
    Ok((String::from_utf8(ran.stdout)?, ran.status.success()))
}

#[test]
fn project_access_and_an_issue_note_read_the_record_the_fleet_shares()
-> Result<(), Box<dyn Error>> {
    let scratch = tracked("shared")?;
    join_fleet(&scratch.0.join("share/conductor"), Some(ELSEWHERE))?;
    let (access, granted) = home(&scratch, &["project", "access", "sample"])?;
    assert!(!granted, "{access}");
    assert!(access.contains("permission denied"), "{access}");
    let (note, noted) = home(
        &scratch,
        &["issue", "note", "sample", "42", "worker", "started"],
    )?;
    assert!(!noted, "{note}");
    assert!(note.contains("tracker denied permission"), "{note}");
    Ok(())
}

#[test]
fn a_fleet_without_the_shared_record_leaves_the_credential_missing()
-> Result<(), Box<dyn Error>> {
    let scratch = tracked("unshared")?;
    join_fleet(&scratch.0.join("share/conductor"), None)?;
    let (access, granted) = home(&scratch, &["project", "access", "sample"])?;
    assert!(!granted, "{access}");
    assert!(access.contains("host credential is missing"), "{access}");
    let (note, noted) = home(
        &scratch,
        &["issue", "note", "sample", "42", "worker", "started"],
    )?;
    assert!(!noted, "{note}");
    assert!(note.contains("host credential is missing"), "{note}");
    Ok(())
}
