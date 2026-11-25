use super::{
    Backend, Backends, Binding, Capability, CredentialState, Refusal,
    Selection, preflight,
};

fn selected() -> Selection {
    Selection {
        adapter: String::from("local-vcs"),
        resource: String::from("/projects/local"),
        credential: None,
        options: String::new(),
    }
}

#[test]
fn an_available_credential_must_have_a_host_reference() {
    let selection = selected();
    let backends = Backends {
        tracker: Some(selection.clone()),
        ..Backends::default()
    };
    let bindings = [Binding {
        backend: Backend::IssueTracker,
        selection,
        capabilities: vec![Capability::ReadIssues],
        credential: CredentialState::Available,
    }];
    assert_eq!(
        preflight(&backends, &bindings, Capability::ReadIssues),
        Err(Refusal::CredentialMissing {
            backend: Backend::IssueTracker,
            reference: None
        })
    );
}

#[test]
fn capturing_a_local_snapshot_does_not_resolve_the_transport_credential()
-> Result<(), String> {
    let selection = Selection {
        credential: Some(super::CredentialRef::try_from(String::from(
            "host/remote",
        ))?),
        ..selected()
    };
    let backends = Backends {
        version_control: Some(selection.clone()),
        ..Backends::default()
    };
    let bindings = [Binding {
        backend: Backend::VersionControl,
        selection,
        capabilities: vec![Capability::CaptureSnapshot],
        credential: CredentialState::Missing,
    }];
    assert_eq!(
        preflight(&backends, &bindings, Capability::CaptureSnapshot),
        Ok(&bindings[0])
    );
    Ok(())
}

#[test]
fn inline_remote_credentials_are_not_project_configuration() {
    let selection = Selection {
        resource: String::from(
            "https://name:synthetic-token@example.test/repo",
        ),
        ..selected()
    };
    assert_eq!(
        selection.validate(Backend::VersionControl),
        Err(Refusal::InvalidConfiguration {
            backend: Backend::VersionControl
        })
    );
}

#[test]
fn project_capabilities_is_a_configuration_query() {
    let words = ["project", "capabilities", "local"].map(String::from);
    assert_eq!(super::requested(&words), Some("local"));
    assert_eq!(super::requested(&words[..2]), None);
    assert_eq!(
        super::requested(&[
            String::from("issue"),
            String::from("new"),
            String::from("local")
        ]),
        None
    );
}

#[test]
fn host_credential_references_cannot_be_paths_outside_the_host_store() {
    for reference in [
        "/etc/token",
        "../token",
        "host/../token",
        "host//token",
        ".",
        "",
    ] {
        assert!(
            super::CredentialRef::try_from(String::from(reference)).is_err(),
            "{reference}"
        );
    }
    assert!(
        super::CredentialRef::try_from(String::from("host/team-token")).is_ok()
    );
}

#[test]
fn independent_backend_configuration_retains_only_host_credential_references()
-> Result<(), String> {
    let text = r#"
name = "local"
title = "Local"
root = "fc27e50efb661d7ef4a45d3edbe0234ff0a6fbd5"
[backends.version_control]
adapter = "local-vcs"
resource = "/projects/local"
[backends.tracker]
adapter = "remote-issues"
resource = "team/issues"
credential = "host/issue-token"
[backends.hosting]
adapter = "build-host"
resource = "team/builds"
"#;
    let project = crate::project::parse("a1b2c3", text)?;
    assert!(project.remote.is_none());
    assert_eq!(
        project
            .backends
            .version_control
            .as_ref()
            .map(|backend| backend.adapter.as_str()),
        Some("local-vcs")
    );
    assert_eq!(
        project
            .backends
            .hosting
            .as_ref()
            .map(|backend| backend.resource.as_str()),
        Some("team/builds")
    );
    assert_eq!(
        project
            .backends
            .tracker
            .as_ref()
            .and_then(|backend| backend.credential.as_ref())
            .map(super::CredentialRef::as_str),
        Some("host/issue-token")
    );
    let inline = text.replace(
        "credential = \"host/issue-token\"",
        "token = \"synthetic-inline-secret\"",
    );
    assert!(crate::project::parse("a1b2c3", &inline).is_err());
    Ok(())
}
