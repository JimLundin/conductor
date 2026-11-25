use std::{
    io::Write,
    path::Path,
    sync::mpsc::{self, Receiver},
    time::Duration,
};

use anyhow::{Context, Result};
use octocrab::{
    Octocrab,
    models::{AppId, InstallationId},
    service::middleware::retry::RetryConfig,
};
use secrecy::ExposeSecret;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct ScopeRequest {
    repositories: Vec<String>,
}

#[derive(Deserialize)]
struct ReadResponse {
    ok: bool,
}
use transport_probe_core::{Failure, Request, failure, header_value, response};

use crate::{git, server::Server};

struct Exchange {
    route: String,
    authorization: String,
    status: u16,
    extra: String,
    body: Value,
    scope: bool,
}

fn exchange(
    route: &str,
    authorization: &str,
    status: u16,
    body: Value,
) -> Exchange {
    Exchange {
        route: route.to_owned(),
        authorization: authorization.to_owned(),
        status,
        extra: String::new(),
        body,
        scope: false,
    }
}

fn token(value: &str, expires: &str) -> Exchange {
    Exchange {
        scope: true,
        ..exchange(
            "POST /app/installations/42/access_tokens ",
            "Bearer ey",
            201,
            json!({"token": value, "expires_at": expires, "permissions": {"checks":"read", "contents":"write"}}),
        )
    }
}

fn handle(request: &Request, expected: &Exchange) -> Result<Vec<u8>> {
    anyhow::ensure!(
        request.header.starts_with(&expected.route),
        "unexpected fixture API route"
    );
    anyhow::ensure!(
        header_value(&request.header, "authorization")
            .context("missing API authentication")?
            .starts_with(&expected.authorization),
        "unexpected fixture API credential"
    );
    if expected.scope {
        let body: ScopeRequest = serde_json::from_slice(&request.body)?;
        anyhow::ensure!(
            body.repositories == ["fixture"],
            "installation repository scope was lost"
        );
    }
    Ok(response(
        expected.status,
        "application/json",
        &expected.extra,
        &serde_json::to_vec(&expected.body)?,
    ))
}

fn server(exchanges: Vec<Exchange>) -> Result<(Server, Receiver<usize>)> {
    let (observed, count) = mpsc::channel();
    let mut index = 0;
    let fixture = Server::start(None, move |request| {
        let reply = handle(
            &request,
            exchanges.get(index).context("unexpected API replay")?,
        )?;
        index += 1;
        observed.send(index)?;
        Ok(reply)
    })?;
    Ok((fixture, count))
}

fn personal(url: &str, token: &str) -> Result<Octocrab> {
    Ok(Octocrab::builder()
        .base_uri(url)?
        .personal_token(token.to_owned())
        .add_retry_config(RetryConfig::None)
        .set_read_timeout(Some(Duration::from_secs(5)))
        .build()?)
}

fn installation(root: &Path, url: &str) -> Result<Octocrab> {
    Ok(Octocrab::builder()
        .base_uri(url)?
        .app_from_rsa_pem(AppId(7), std::fs::read(root.join("app.key"))?)?
        .add_retry_config(RetryConfig::None)
        .set_read_timeout(Some(Duration::from_secs(5)))
        .build()?
        .installation_builder(InstallationId(42))
        .repositories(vec!["fixture".to_owned()])
        .build()?)
}

async fn read(crab: &Octocrab) -> octocrab::Result<ReadResponse> {
    crab.get("/fixture", None::<&()>).await
}

fn classify(error: &octocrab::Error) -> Result<Failure> {
    let octocrab::Error::GitHub { source, .. } = error else {
        anyhow::bail!("expected typed GitHub HTTP error");
    };
    let headers = source
        .headers
        .as_ref()
        .context("missing response headers")?;
    Ok(failure(
        source.status_code.as_u16(),
        headers
            .get("x-ratelimit-remaining")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok()),
        headers.contains_key("retry-after"),
    ))
}

async fn personal_cases() -> Result<()> {
    let checks = "/repos/fixture/fixture/commits/abc/check-runs";
    let mut limited = exchange(
        "GET /fixture ",
        "Bearer personal",
        403,
        json!({"message":"API rate limit exceeded"}),
    );
    "X-RateLimit-Remaining: 0\r\nRetry-After: 1\r\n"
        .clone_into(&mut limited.extra);
    let (fixture, count) = server(vec![
        exchange(
            &format!("GET {checks}"),
            "Bearer personal",
            403,
            json!({"message":"Resource not accessible by personal access token"}),
        ),
        exchange(
            "GET /fixture ",
            "Bearer personal",
            401,
            json!({"message":"Bad credentials"}),
        ),
        limited,
        exchange(
            "POST /fixture ",
            "Bearer personal",
            503,
            json!({"message":"fixture unavailable"}),
        ),
    ])?;
    let crab = personal(&format!("http://{}", fixture.address), "personal")?;
    let denied = crab
        .checks("fixture", "fixture")
        .list_check_runs_for_git_ref(octocrab::params::repos::Commitish(
            "abc".to_owned(),
        ))
        .send()
        .await;
    anyhow::ensure!(
        classify(
            &denied
                .err()
                .context("private checks unexpectedly allowed")?
        )? == Failure::Access,
        "permission classification"
    );
    anyhow::ensure!(
        classify(
            &read(&crab)
                .await
                .err()
                .context("PAT 401 unexpectedly accepted")?
        )? == Failure::Authentication,
        "authentication classification"
    );
    anyhow::ensure!(
        classify(
            &read(&crab)
                .await
                .err()
                .context("rate limit unexpectedly accepted")?
        )? == Failure::RateLimited,
        "rate limit classification"
    );
    let write: octocrab::Result<ReadResponse> = crab
        .post("/fixture", Some(&json!({"body":"fixture"})))
        .await;
    anyhow::ensure!(write.is_err(), "503 mutation accepted");
    anyhow::ensure!(
        count.try_iter().last() == Some(4),
        "personal request replayed"
    );
    fixture.finish()
}

async fn refresh_cases(root: &Path, workspace: &Path) -> Result<()> {
    let (fixture, count) = server(vec![
        token("expired", "2000-01-01T00:00:00Z"),
        token("fixture-token", "2099-01-01T00:00:00Z"),
        exchange(
            "GET /fixture ",
            "Bearer fixture-token",
            401,
            json!({"message":"Bad credentials"}),
        ),
        token("refreshed", "2099-01-01T00:00:00Z"),
        exchange("GET /fixture ", "Bearer refreshed", 200, json!({"ok":true})),
        exchange(
            "GET /repos/fixture/fixture/commits/abc/check-runs",
            "Bearer refreshed",
            200,
            json!({"total_count":0,"check_runs":[]}),
        ),
    ])?;
    let crab = installation(root, &format!("http://{}", fixture.address))?;
    let expired = crab.installation_token().await?;
    anyhow::ensure!(
        expired.expose_secret() == "expired",
        "fixture issuance mismatch"
    );
    let fresh = crab.installation_token().await?;
    let cached = crab.installation_token().await?;
    anyhow::ensure!(
        fresh.expose_secret() == cached.expose_secret(),
        "valid token not cached"
    );
    anyhow::ensure!(
        count.try_iter().last() == Some(2),
        "unexpected mint count"
    );
    let local = git2::Repository::init_bare(workspace.join("app-local"))?;
    let remote_path = workspace.join("app-remote");
    git2::Repository::init_bare(&remote_path)?;
    let candidate = git::commit(&local, None, "installation token Git push")?;
    let endpoint = git::Endpoint::start(
        root,
        remote_path,
        false,
        None,
        transport_probe_core::BASIC,
    )?;
    let pushed = git::push(
        &local,
        &endpoint.url(),
        git2::Oid::ZERO_SHA1,
        candidate,
        fresh.expose_secret(),
    )?;
    anyhow::ensure!(
        transport_probe_core::accepted(pushed.result.is_ok(), &pushed.statuses),
        "refreshed installation token not accepted by HTTPS fixture"
    );
    endpoint.server.finish()?;
    anyhow::ensure!(read(&crab).await?.ok, "installation 401 refresh failed");
    anyhow::ensure!(
        count.try_iter().last() == Some(5),
        "401 did not refresh exactly once"
    );
    let checks = crab
        .checks("fixture", "fixture")
        .list_check_runs_for_git_ref(octocrab::params::repos::Commitish(
            "abc".to_owned(),
        ))
        .send()
        .await?;
    anyhow::ensure!(
        checks.total_count == 0,
        "typed Checks response did not decode"
    );
    anyhow::ensure!(
        count.try_iter().last() == Some(6),
        "unexpected Checks read count"
    );
    fixture.finish()
}

async fn write_cases(root: &Path) -> Result<()> {
    let (fixture, count) = server(vec![
        token("write-token", "2099-01-01T00:00:00Z"),
        exchange(
            "POST /fixture ",
            "Bearer write-token",
            401,
            json!({"message":"Bad credentials"}),
        ),
        exchange(
            "POST /fixture ",
            "Bearer write-token",
            503,
            json!({"message":"fixture unavailable"}),
        ),
        exchange("POST /fixture ", "Bearer write-token", 0, json!(null)),
    ])?;
    let url = format!("http://{}", fixture.address);
    let installer = installation(root, &url)?;
    let current = installer.installation_token().await?;
    let writer = personal(&url, current.expose_secret())?;
    let unauthorized: octocrab::Result<ReadResponse> = writer
        .post("/fixture", Some(&json!({"operation":"first"})))
        .await;
    anyhow::ensure!(unauthorized.is_err(), "unauthorized mutation accepted");
    let unavailable: octocrab::Result<ReadResponse> = writer
        .post("/fixture", Some(&json!({"operation":"second"})))
        .await;
    anyhow::ensure!(unavailable.is_err(), "unavailable mutation accepted");
    let disconnected: octocrab::Result<ReadResponse> = writer
        .post("/fixture", Some(&json!({"operation":"third"})))
        .await;
    anyhow::ensure!(disconnected.is_err(), "lost-response mutation accepted");
    anyhow::ensure!(
        count.try_iter().last() == Some(4),
        "token-authenticated mutation replayed"
    );
    fixture.finish()
}

pub async fn cases(root: &Path, workspace: &Path) -> Result<()> {
    personal_cases().await?;
    refresh_cases(root, workspace).await?;
    write_cases(root).await?;
    std::io::stdout().write_all(b"Octocrab: PAT permissions/auth/rate limit, installation expiry/cache/401 refresh/scope, refreshed-token Git HTTPS, and mutation no-replay passed\n")?;
    Ok(())
}
