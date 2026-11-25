use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
};

use anyhow::{Context, Result};
use git2::{
    Cred, CredentialType, Oid, PushOptions, RemoteCallbacks, Repository,
    Signature, Time,
};
use transport_probe_core::{
    BASIC, Credential, GitAction, REF, Request, TOKEN, Transport, Update, ZERO,
    accepted, advertisement, credential, git_action, negotiation_refusal,
    refusal, report, response,
};

use crate::server::{Server, tls};

#[derive(Clone, Copy, Debug, Default)]
pub struct Evidence {
    pub posts: usize,
    pub challenges: usize,
}

pub struct Endpoint {
    path: PathBuf,
    pub evidence: Receiver<Evidence>,
    pub server: Server,
}

fn current(repo: &Repository) -> Oid {
    repo.refname_to_id(REF).unwrap_or(Oid::ZERO_SHA1)
}

fn install(repo: &Repository, update: &Update) -> Result<()> {
    let odb = repo.odb()?;
    let mut pack = odb.packwriter()?;
    pack.write_all(&update.pack)?;
    pack.commit()?;
    repo.find_commit(Oid::from_str(&update.new)?)?;
    repo.reference(
        &update.name,
        Oid::from_str(&update.new)?,
        true,
        "fixture receive",
    )?;
    Ok(())
}

fn receive(
    repo: &Repository,
    update: &Update,
    reject: bool,
    race: Option<Oid>,
) -> Result<Vec<u8>> {
    if let Some(competitor) = race {
        repo.reference(REF, competitor, true, "fixture competing writer")?;
    }
    let refusal = refusal(reject, current(repo) != Oid::from_str(&update.old)?);
    if refusal.is_none() {
        install(repo, update)?;
    }
    Ok(response(
        200,
        "application/x-git-receive-pack-result",
        "",
        report(refusal).as_bytes(),
    ))
}

fn handle(
    repo: &Repository,
    request: &Request,
    reject: bool,
    race: Option<Oid>,
    evidence: &mut Evidence,
    authorization: &str,
) -> Result<Vec<u8>> {
    match git_action(request, authorization) {
        GitAction::Challenge => {
            evidence.challenges += 1;
            Ok(response(
                401,
                "text/plain",
                "WWW-Authenticate: Basic realm=\"fixture\"\r\n",
                b"authentication required",
            ))
        }
        GitAction::Advertise => Ok(response(
            200,
            "application/x-git-receive-pack-advertisement",
            "",
            advertisement(&current(repo).to_string(), true).as_bytes(),
        )),
        GitAction::Receive(update) => {
            evidence.posts += 1;
            receive(repo, &update, reject, race)
        }
        GitAction::Missing => {
            Ok(response(404, "text/plain", "", b"missing fixture route"))
        }
    }
}

impl Endpoint {
    pub fn start(
        root: &Path,
        path: PathBuf,
        reject: bool,
        race: Option<Oid>,
        authorization: &str,
    ) -> Result<Self> {
        let repo = Repository::open_bare(&path)?;
        let (observed, evidence) = mpsc::channel();
        let mut server_evidence = Evidence::default();
        let expected_authorization = authorization.to_owned();
        let server = Server::start(Some(tls(root)?), move |request| {
            let reply = handle(
                &repo,
                &request,
                reject,
                race,
                &mut server_evidence,
                &expected_authorization,
            )?;
            observed.send(server_evidence)?;
            Ok(reply)
        })?;
        Ok(Self {
            path,
            evidence,
            server,
        })
    }

    pub fn url(&self) -> String {
        format!("https://{}/repo.git", self.server.address)
    }

    pub fn current(&self) -> Result<Oid> {
        Ok(current(&Repository::open_bare(&self.path)?))
    }
}

pub struct PushResult {
    pub result: Result<(), git2::Error>,
    pub statuses: Vec<(String, Option<String>)>,
    pub negotiations: Vec<(String, String, String)>,
    pub credentials: usize,
}

pub fn callbacks<'a>(
    token: &'a str,
    count: &'a mut usize,
) -> RemoteCallbacks<'a> {
    let mut callbacks = RemoteCallbacks::new();
    callbacks.credentials(move |_, _, allowed| {
        *count += 1;
        match credential(
            Transport::Https,
            false,
            false,
            allowed.contains(CredentialType::USER_PASS_PLAINTEXT),
            *count,
        ) {
            Credential::Token => {
                Cred::userpass_plaintext("x-access-token", token)
            }
            _ => Err(git2::Error::from_str(
                "fixture credentials exhausted or unsupported",
            )),
        }
    });
    callbacks
}

pub fn push(
    repo: &Repository,
    url: &str,
    expected: Oid,
    candidate: Oid,
    token: &str,
) -> Result<PushResult> {
    let mut statuses = Vec::new();
    let mut negotiations = Vec::new();
    let mut credentials = 0;
    let result = {
        let mut hooks = callbacks(token, &mut credentials);
        hooks.push_negotiation(|updates| {
            negotiations = updates
                .iter()
                .map(|update| {
                    Ok((
                        update.src().to_string(),
                        update.dst().to_string(),
                        update.dst_refname()?.to_owned(),
                    ))
                })
                .collect::<Result<_, git2::Error>>()?;
            negotiation_refusal(
                &expected.to_string(),
                &candidate.to_string(),
                &negotiations,
            )
            .map_or(Ok(()), |reason| Err(git2::Error::from_str(reason)))
        });
        hooks.push_update_reference(|name, status| {
            statuses.push((name.to_owned(), status.map(str::to_owned)));
            Ok(())
        });
        repo.remote_anonymous(url)?.push(
            &[&format!("{candidate}:{REF}")],
            Some(PushOptions::new().remote_callbacks(hooks)),
        )
    };
    Ok(PushResult {
        result,
        statuses,
        negotiations,
        credentials,
    })
}

pub fn commit(
    repo: &Repository,
    parent: Option<Oid>,
    text: &str,
) -> Result<Oid> {
    let mut tree = repo.treebuilder(None)?;
    tree.insert("fixture.txt", repo.blob(text.as_bytes())?, 0o100_644)?;
    let signature = Signature::new(
        "Transport fixture",
        "fixture@example.invalid",
        &Time::new(1_700_000_000, 0),
    )?;
    let parent_commit = parent.map(|oid| repo.find_commit(oid)).transpose()?;
    Ok(repo.commit(
        None,
        &signature,
        &signature,
        text,
        &repo.find_tree(tree.write()?)?,
        &parent_commit.iter().collect::<Vec<_>>(),
    )?)
}

fn verify_acceptance(
    result: &PushResult,
    endpoint: &Endpoint,
    candidate: Oid,
) -> Result<()> {
    anyhow::ensure!(
        accepted(result.result.is_ok(), &result.statuses),
        "missing ref acceptance: transport={:?}, statuses={:?}, credentials={}, server={:?}",
        result.result,
        result.statuses,
        result.credentials,
        endpoint
            .evidence
            .try_iter()
            .last()
            .context("missing fixture evidence")?
    );
    anyhow::ensure!(
        endpoint.current()? == candidate,
        "remote did not receive exact commit"
    );
    Ok(())
}

pub fn cases(root: &Path, workspace: &Path) -> Result<()> {
    let local = Repository::init_bare(workspace.join("local"))?;
    let remote_path = workspace.join("remote");
    let remote = Repository::init_bare(&remote_path)?;
    let first = commit(&local, None, "first")?;
    let second = commit(&local, Some(first), "second")?;
    let third = commit(&local, Some(second), "third")?;
    let competitor = commit(&remote, None, "competitor")?;
    let endpoint =
        Endpoint::start(root, remote_path.clone(), false, None, BASIC)?;
    let created = push(&local, &endpoint.url(), Oid::ZERO_SHA1, first, TOKEN)?;
    verify_acceptance(&created, &endpoint, first)?;
    anyhow::ensure!(created.credentials > 0, "HTTPS callback not called");
    let advanced = push(&local, &endpoint.url(), first, second, TOKEN)?;
    verify_acceptance(&advanced, &endpoint, second)?;
    let stale = push(&local, &endpoint.url(), first, third, TOKEN)?;
    anyhow::ensure!(stale.result.is_err(), "stale negotiation accepted");
    anyhow::ensure!(
        stale.negotiations.first().context("missing negotiation")?.0
            == second.to_string(),
        "wrong advertised old OID"
    );
    anyhow::ensure!(
        endpoint
            .evidence
            .try_iter()
            .last()
            .context("missing fixture evidence")?
            .posts
            == 2,
        "stale negotiation sent a POST"
    );
    let bad_auth = push(&local, &endpoint.url(), second, third, "incorrect")?;
    anyhow::ensure!(bad_auth.result.is_err(), "incorrect credential accepted");
    anyhow::ensure!(
        bad_auth.credentials <= 3,
        "authentication callback unbounded"
    );
    endpoint.server.finish()?;
    certificate_case(root, &local, &remote_path, second, third)?;
    rejected_case(root, &local, &remote_path, second, third, None)?;
    rejected_case(root, &local, &remote_path, second, third, Some(competitor))?;
    std::io::stdout().write_all(b"HTTPS: create, fast-forward, stale negotiation, bounded auth failure, TLS hostname refusal, server policy refusal and post-advertisement race passed\n")?;
    Ok(())
}

fn tls_control(repo: &Repository, endpoint: &Endpoint) -> Result<()> {
    let mut credentials = 0;
    repo.remote_anonymous(&endpoint.url())?
        .connect_auth(
            git2::Direction::Push,
            Some(callbacks(TOKEN, &mut credentials)),
            None,
        )
        .map(drop)?;
    anyhow::ensure!(
        credentials > 0,
        "trusted TLS control did not authenticate"
    );
    Ok(())
}

fn certificate_case(
    root: &Path,
    repo: &Repository,
    remote: &Path,
    expected: Oid,
    candidate: Oid,
) -> Result<()> {
    let endpoint =
        Endpoint::start(root, remote.to_owned(), false, None, BASIC)?;
    tls_control(repo, &endpoint)?;
    let bad_host = push(
        repo,
        &endpoint.url().replace("127.0.0.1", "localhost"),
        expected,
        candidate,
        TOKEN,
    )?;
    let error = bad_host
        .result
        .as_ref()
        .err()
        .context("wrong TLS hostname accepted")?;
    anyhow::ensure!(
        error.class() == git2::ErrorClass::Ssl,
        "hostname control did not fail in TLS: {error}"
    );
    anyhow::ensure!(
        bad_host.credentials == 0,
        "credential sent before TLS refusal"
    );
    anyhow::ensure!(
        bad_host.negotiations.is_empty(),
        "untrusted TLS reached push negotiation"
    );
    tls_control(repo, &endpoint)?;
    anyhow::ensure!(
        endpoint
            .evidence
            .try_iter()
            .last()
            .context("missing TLS fixture evidence")?
            .posts
            == 0,
        "TLS controls sent a push body"
    );
    anyhow::ensure!(
        endpoint.current()? == expected,
        "TLS refusal changed remote ref"
    );
    writeln!(
        std::io::stdout(),
        "TLS hostname refusal: {:?}/{:?}, zero credentials and push bodies",
        error.code(),
        error.class()
    )?;
    endpoint.server.finish()
}

fn rejected_case(
    root: &Path,
    repo: &Repository,
    remote: &Path,
    expected: Oid,
    candidate: Oid,
    race: Option<Oid>,
) -> Result<()> {
    let endpoint =
        Endpoint::start(root, remote.to_owned(), race.is_none(), race, BASIC)?;
    let refused = push(repo, &endpoint.url(), expected, candidate, TOKEN)?;
    anyhow::ensure!(
        refused.result.is_ok(),
        "refusal was not transport success"
    );
    anyhow::ensure!(
        !accepted(true, &refused.statuses),
        "refusal interpreted as success"
    );
    anyhow::ensure!(
        refused.statuses.first().context("no status")?.1.is_some(),
        "no server rejection reason"
    );
    anyhow::ensure!(
        endpoint.current()? == race.unwrap_or(expected),
        "rejected ref changed"
    );
    anyhow::ensure!(
        endpoint
            .evidence
            .try_iter()
            .last()
            .context("missing fixture evidence")?
            .posts
            == 1,
        "refusal was local"
    );
    endpoint.server.finish()
}

pub fn empty_advertisement() -> String {
    advertisement(ZERO, false)
}
