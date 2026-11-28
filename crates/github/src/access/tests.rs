use alloc::{string::String, vec::Vec};
use home::ProjectId;
use home::backend::{Backends, Selection};
use home::project::Project;

use super::{
    AccessError, Repositories, Repository, TokenKind, validate_bearer,
};

fn tracked(id: u64, adapter: &str, resource: &str) -> Project {
    Project {
        id: ProjectId::new(id),
        name: String::from("project"),
        title: String::from("Project"),
        remote: None,
        root: String::from("."),
        profiles: Vec::new(),
        provides: Vec::new(),
        backends: Backends {
            tracker: Some(Selection {
                adapter: String::from(adapter),
                resource: String::from(resource),
                credential: None,
                options: String::new(),
            }),
            ..Backends::default()
        },
    }
}

#[test]
fn each_repository_belongs_to_the_project_whose_github_tracker_names_it()
-> Result<(), AccessError> {
    let repositories = Repositories::of(&[
        tracked(1, "github", "owner/work"),
        tracked(2, "github", "Owner/Standards"),
        tracked(3, "linear", "owner/linear"),
    ]);
    assert_eq!(
        repositories.project(&Repository::parse("owner/standards")?),
        Some(ProjectId::new(2))
    );
    assert_eq!(
        repositories.repository(ProjectId::new(1)),
        Some(&Repository::parse("owner/work")?)
    );
    assert_eq!(
        repositories.project(&Repository::parse("owner/linear")?),
        None
    );
    assert_eq!(repositories.repository(ProjectId::new(3)), None);
    Ok(())
}

#[test]
fn only_a_single_github_repository_can_receive_its_credential()
-> Result<(), AccessError> {
    let repository = Repository::parse("Example/Issue.Board")?;
    assert!(repository.matches("example/issue.board"));
    assert!(!repository.matches("example/other"));
    for resource in [
        "example/..",
        "/example/repo",
        "a/b/c",
        "a/b?x",
        "a/b#x",
        "a/b%2f..",
        "a/b\\c",
        "https://elsewhere/a/b",
    ] {
        assert_eq!(
            Repository::parse(resource),
            Err(AccessError::InvalidConfiguration)
        );
    }
    Ok(())
}

#[test]
fn checks_require_the_selected_credentials_access() {
    assert!(!TokenKind::FineGrained.permits_checks(Some(true)));
    assert!(!TokenKind::FineGrained.permits_checks(None));
    assert!(TokenKind::FineGrained.permits_checks(Some(false)));
    assert!(TokenKind::Installation.permits_checks(Some(true)));
    assert!(TokenKind::OAuth.permits_checks(Some(true)));
}

#[test]
fn authentication_permission_rate_and_availability_failures_are_distinct() {
    assert_eq!(
        AccessError::response(401, false, false),
        AccessError::AuthenticationRefused
    );
    assert_eq!(
        AccessError::response(403, false, false),
        AccessError::PermissionDenied
    );
    assert_eq!(
        AccessError::response(403, true, false),
        AccessError::RateLimited
    );
    assert_eq!(
        AccessError::response(403, false, true),
        AccessError::RateLimited
    );
    assert_eq!(
        AccessError::response(404, false, false),
        AccessError::RepositoryUnavailable
    );
    assert_eq!(
        AccessError::response(503, false, false),
        AccessError::RequestFailed
    );
}

#[test]
fn bearer_tokens_require_an_unbroken_token_with_optional_trailing_padding() {
    for token in ["gho_mock", "a.-_~+/=="] {
        assert!(validate_bearer(token).is_ok());
    }
    for token in ["", "=", "a=b", "a b", "a\rb", "a\nb", "a:b", "é"] {
        assert_eq!(
            validate_bearer(token),
            Err(AccessError::InvalidConfiguration)
        );
    }
}
