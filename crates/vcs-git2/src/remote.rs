use git2::{CertificateCheckStatus, Cred, CredentialType, RemoteCallbacks};
use secrecy::ExposeSecret as _;
use vcs::remote::{Challenge, challenge};

use project_runtime::transport::{Credentials, Method};

#[must_use]
pub fn callbacks(credential: &Credentials) -> RemoteCallbacks<'_> {
    let mut callbacks = RemoteCallbacks::new();
    let mut count = 0u8;
    callbacks.credentials(move |url, _, allowed| {
        count = count.saturating_add(1);
        respond(credential, url, allowed, count)
    });
    if let Method::Agent { host, key, .. } = credential.method() {
        callbacks.certificate_check(move |cert, name| {
            let actual =
                cert.as_hostkey().and_then(git2::cert::CertHostkey::hostkey);
            (name.eq_ignore_ascii_case(host) && actual == Some(key.as_slice()))
                .then_some(CertificateCheckStatus::CertificateOk)
                .ok_or_else(|| {
                    git2::Error::from_str(
                        "SSH host key does not match configured host",
                    )
                })
        });
    }
    callbacks
}

fn respond(
    credential: &Credentials,
    url: &str,
    allowed: CredentialType,
    count: u8,
) -> Result<Cred, git2::Error> {
    let kinds = (
        allowed.contains(CredentialType::USERNAME),
        allowed.contains(CredentialType::SSH_KEY),
        allowed.contains(CredentialType::USER_PASS_PLAINTEXT),
    );
    match (
        credential.method(),
        challenge(
            credential.url(),
            url,
            matches!(credential.method(), Method::Agent { .. }),
            kinds,
            count,
        ),
    ) {
        (Method::Token(token), Challenge::Token) => {
            Cred::userpass_plaintext("x-access-token", token.expose_secret())
        }
        (Method::Agent { user, .. }, Challenge::Username) => {
            Cred::username(user)
        }
        (Method::Agent { user, .. }, Challenge::Agent) => {
            Cred::ssh_key_from_agent(user)
        }
        _ => Err(git2::Error::from_str("credential refused for this remote")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::SecretString;
    #[test]
    fn token_is_returned_only_for_its_configured_repository_and_bounded_challenge()
     {
        let url = "https://github.com/owner/project.git";
        let credential = Credentials::https(
            String::from(url),
            SecretString::from("synthetic"),
        );
        let challenge_type = CredentialType::USER_PASS_PLAINTEXT;
        assert!(respond(&credential, url, challenge_type, 1).is_ok());
        for wrong in [
            "http://github.com/owner/project.git",
            "https://other.test/owner/project.git",
            "https://github.com/other/repo.git",
        ] {
            assert!(respond(&credential, wrong, challenge_type, 1).is_err());
        }
        assert!(respond(&credential, url, CredentialType::SSH_KEY, 1).is_err());
        assert!(respond(&credential, url, challenge_type, 3).is_err());
    }
}
