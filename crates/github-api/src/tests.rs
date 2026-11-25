use super::*;
use github::access::AccessError;

#[cfg(unix)]
#[tokio::test]
async fn a_project_cannot_select_another_repositorys_host_credential()
-> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("project-credentials.toml");
    std::fs::write(
        &path,
        r#"
["host/work"]
repository = "owner/work"
["host/work".source]
kind = "fine_grained"
token = "controlled-test-token"
"#,
    )?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    let host = HostCredentials::new(dir.path());
    assert!(matches!(
        host.connect(&selection("other/repo")),
        Err(AccessError::PermissionDenied)
    ));
    assert!(matches!(
        host.connect(&selection("owner/../repo")),
        Err(AccessError::InvalidConfiguration)
    ));
    assert!(host.connect(&selection("owner/work")).is_ok());
    let original = std::fs::read_to_string(&path)?;
    std::fs::write(
        &path,
        original.replace("repository =", "expires_unix = 1\nrepository ="),
    )?;
    assert!(matches!(
        host.connect(&selection("owner/work")),
        Err(AccessError::Expired)
    ));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640))?;
    assert!(matches!(
        host.connect(&selection("owner/work")),
        Err(AccessError::CredentialStoreUnavailable)
    ));
    std::fs::remove_file(&path)?;
    assert!(matches!(
        host.connect(&selection("owner/work")),
        Err(AccessError::CredentialMissing)
    ));
    Ok(())
}

fn selection(resource: &str) -> home::backend::Selection {
    home::backend::Selection {
        adapter: String::from("github"),
        resource: String::from(resource),
        credential: home::backend::CredentialRef::try_from(String::from(
            "host/work",
        ))
        .ok(),
        options: String::new(),
    }
}

use crate::credential::Source;
use github::access::{Observation, Read, Repository};
use home::backend::Backend;
use http_body_util::{BodyExt as _, Full};
use hyper::{
    Request, Response,
    body::{Bytes, Incoming},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use secrecy::SecretString;
use std::convert::Infallible;

type Reply = (u16, &'static str);
const META: &str = r#"{"id":7,"name":"work","full_name":"owner/work","url":"https://api.github.com/repos/owner/work","private":true,"default_branch":"main"}"#;

async fn server(
    expected: Vec<(&'static str, &'static str, Reply)>,
) -> anyhow::Result<(String, tokio::task::JoinHandle<anyhow::Result<()>>)> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let url = format!("http://{}", listener.local_addr()?);
    let task = tokio::spawn(async move {
        for (path, auth, (status, response)) in expected {
            let (socket, _) = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                listener.accept(),
            )
            .await??;
            let serve =
                service_fn(move |request: Request<Incoming>| async move {
                    assert_eq!(request.uri().path(), path);
                    assert!(
                        request
                            .headers()
                            .get("authorization")
                            .and_then(|value| value.to_str().ok())
                            .is_some_and(|value| value.starts_with(auth))
                    );
                    let body = request
                        .into_body()
                        .collect()
                        .await
                        .unwrap_or_default()
                        .to_bytes();
                    if path.contains("/access_tokens") {
                        let scope =
                            serde_json::from_slice::<Scoped>(&body).ok();
                        assert_eq!(
                            scope.map(|read| read.repositories),
                            Some(vec![String::from("work")])
                        );
                    }
                    let mut result = Response::new(Full::new(
                        Bytes::from_static(response.as_bytes()),
                    ));
                    *result.status_mut() =
                        hyper::StatusCode::from_u16(status).unwrap_or_default();
                    result.headers_mut().insert(
                        hyper::header::CONNECTION,
                        hyper::header::HeaderValue::from_static("close"),
                    );
                    Ok::<_, Infallible>(result)
                });
            http1::Builder::new()
                .serve_connection(TokioIo::new(socket), serve)
                .await?;
        }
        Ok(())
    });
    Ok((url, task))
}

#[derive(serde::Deserialize)]
struct Scoped {
    repositories: Vec<String>,
}

#[tokio::test]
async fn private_fine_grained_access_probes_statuses_and_workflows_without_submitting_a_check_request()
-> anyhow::Result<()> {
    let auth = "Bearer fixture";
    let (base, finished) = server(vec![
        ("/repos/owner/work", auth, (200, META)),
        ("/repos/owner/work/commits/main/statuses", auth, (200, "[]")),
        (
            "/repos/owner/work/actions/runs",
            auth,
            (403, r#"{"message":"not permitted"}"#),
        ),
    ])
    .await?;
    let client = Client::new(
        Repository::parse("owner/work")?,
        Source::FineGrained {
            token: SecretString::from("fixture"),
        },
        &base,
    )?;
    let observed = client.inspect(Backend::RepositoryHosting).await;
    assert_eq!(
        observed,
        [
            Observation {
                read: Read::Statuses,
                result: Ok(())
            },
            Observation {
                read: Read::Checks,
                result: Err(AccessError::PermissionDenied)
            },
            Observation {
                read: Read::Workflows,
                result: Err(AccessError::PermissionDenied)
            },
        ]
    );
    finished.await??;
    Ok(())
}

#[tokio::test]
async fn scoped_installation_refresh_supplies_the_new_token_to_git()
-> anyhow::Result<()> {
    let pem = openssl::rsa::Rsa::generate(2048)?.private_key_to_pem()?;
    let source = Source::Installation {
        app_id: 9,
        installation_id: 42,
        private_key: String::from_utf8(pem)?.into(),
    };
    let expired = r#"{"token":"old","expires_at":"2000-01-01T00:00:00Z","permissions":{"contents":"read"}}"#;
    let refreshed = r#"{"token":"fresh","expires_at":"2099-01-01T00:00:00Z","permissions":{"contents":"read"}}"#;
    let path = "/app/installations/42/access_tokens";
    let (url, finished) = server(vec![
        (path, "Bearer ey", (201, expired)),
        (path, "Bearer ey", (201, refreshed)),
    ])
    .await?;
    let client = Client::new(Repository::parse("owner/work")?, source, &url)?;
    let _first = client.git().await?;
    let prepared = client.git().await?;
    assert_eq!(prepared.url(), "https://github.com/owner/work.git");
    let project_runtime::transport::Method::Token(value) = prepared.method()
    else {
        return Err(anyhow::anyhow!("expected a refreshed Git credential"));
    };
    assert!(secrecy::ExposeSecret::expose_secret(value).eq("fresh"));
    finished.await??;
    Ok(())
}
