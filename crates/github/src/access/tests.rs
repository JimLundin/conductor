use super::{AccessError, Repository, TokenKind};

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
