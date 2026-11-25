# Credential and transport probe

Decision `0v2crv9w`, signed-off spec `0v265ymj`. This standalone workspace
uses git2 `0.21.0`, libgit2 `1.9.7` and Octocrab `0.54.9`. It changes no
production dependency or adapter API. The [local Git probe](../vcs-probe/README.md)
remains the evidence for local repository operations.

Git2's direct HTTPS and SSH-agent credentials work in the controlled local
fixtures. Octocrab refreshes scoped installation tokens, and those tokens
can authenticate Git HTTPS. An installation-token preflight followed by a
token-authenticated writer with retries disabled keeps mutation replay
under the caller's control. This selects the implementation method;
configured access and the remaining activation checks still govern
production replacement.

## Run

From Conductor's root:

```sh
cargo build --locked --manifest-path docs/experiments/transport-probe/Cargo.toml
bash docs/experiments/transport-probe/run.sh
cargo test --locked --workspace --manifest-path docs/experiments/transport-probe/Cargo.toml
cargo clippy --locked --workspace --all-targets --all-features --manifest-path docs/experiments/transport-probe/Cargo.toml
cargo build --locked -p transport-probe-core --all-features --target wasm32v1-none --manifest-path docs/experiments/transport-probe/Cargo.toml
cargo fmt --all --check --manifest-path docs/experiments/transport-probe/Cargo.toml
```

The runner requires OpenSSL, Python 3, OpenSSH's `ssh-keygen`, `ssh-agent`,
`ssh-add` and `sshd`. It creates temporary keys, a self-signed TLS certificate,
a restricted SSH daemon and a separate agent. Every listener binds only
`127.0.0.1`. Its exit trap terminates the daemon and agent and removes the
fixture directory under the checkout's top-level `target/tp.*`. It reads no
existing secret file and changes no system
SSH configuration. The Rust binary and the forced SSH command execute no
Git, gh or credential-helper process. OpenSSH supplies the disposable server
and agent, not the Git client transport.

HTTPS uses libgit2's OpenSSL verification with the fixture certificate in
`SSL_CERT_FILE`. SSH uses `Cred::ssh_key_from_agent` and compares the raw
server host key with the fixture's generated key. This is an explicit trust
fixture, not an OpenSSH configuration compatibility test.

The receive-pack fixture supports one SHA-1 branch, one command and a small
pack. Successful publication ingests that pack through libgit2 and writes
the target ref. Rejection emits the protocol's `ng` status with HTTP 200
and leaves that ref unchanged. The competing-writer fixture changes the
ref after advertising it, before processing the client's old-OID command.
It is not a general Git server.

## Adapter choices

Use the same host credential source for Git HTTPS and GitHub API requests.
Resolve a personal token directly, or obtain an installation token through
Octocrab's scoped installation client. Call `installation_token()` before
an operation, or `installation_token_with_buffer()` with an appropriate
operation allowance. The default buffer is 30 seconds. Hand the token to
`Cred::userpass_plaintext` with the GitHub username `x-access-token`.
Git2's credential callback is synchronous; acquire refreshed credentials
before entering blocking Git work. Bound rejected credential attempts.

Use a token-authenticated Octocrab client with `RetryConfig::None` for
mutations after installation-token preflight. Keep Octocrab's installation
client as the token source. The SDK's installation authentication handler
refreshes and replays one request after a definitive HTTP 401 independently
of the general retry setting. A 401 means authentication was refused;
a lost response or HTTP 5xx leaves a mutation's effects uncertain. A token
writer makes the decision to retry after authentication refusal explicit
and does not repeat uncertain operations. This uses the existing SDK and
adds no authentication service.

Git2's negotiation callback must compare all of the intended destination,
expected advertised old OID and exact candidate OID. `PushUpdate::src()` is
the old remote OID; `dst()` is the new local OID. Do not infer those meanings
from the method names. Use a refspec without `+`. Require the expected
ref's successful `push_update_reference` status as well as `Remote::push`
success. Missing status evidence remains unconfirmed. After an uncertain
push, reconcile the ref and objects before retrying.

SSH support requires an explicit username, an available agent and verified
host trust. This probe exercises an Ed25519 key in a Unix-domain OpenSSH
agent and an explicitly pinned server key. It does not establish parity
with OpenSSH aliases, configuration files, forwarding, jump hosts, hardware
keys or Windows agents.

## Credential permissions

Permissions remain properties of the configured credential and repository.
A successful controlled response does not prove access to a live repository.

| Configuration | Intended support and required evidence |
|---|---|
| Personal token for HTTPS Git | Direct user/password callback. Fine-grained tokens need Contents read or write for the corresponding operation; classic tokens use repository scopes. Expiration or revocation requires replacement by the existing host credential source. A PAT has no installation-token refresh. |
| Fine-grained personal token for the current queue | Issues, Contents, Commit statuses and Actions as required by the requested operation. Required private Checks API access is unavailable under the approved contract; refuse that requirement. |
| Classic personal token for private Checks reads | The endpoint documents `repo` scope. Access still depends on repository and organization policy. This is distinct from the fine-grained PAT limitation. |
| GitHub App installation | Use a repository-scoped installation client and refresh before use. Contents permissions govern Git; Issues, Commit statuses, Actions and Checks permissions govern the corresponding API calls. An App installation with Checks read is the selected configuration when private Checks API observations are required. |
| SSH agent | Git transport only. GitHub API operations still need a separately configured personal token or App installation. |

The documentation fetched on 2026-11-25 confirms the fine-grained PAT gap.
The token guide lists "Using fine-grained personal access token to call the
Checks API" among its limitations. The List check runs endpoint names only
GitHub App user access tokens and GitHub App installation access tokens in
its fine-grained token list, followed by Checks read permission. It
separately documents classic PATs with `repo` scope for private repositories.
This distinction does not replace a configured permission/access check.

A 401 is authentication refusal. A 403 without rate-limit evidence is an
access refusal; 403 with an exhausted rate limit or `Retry-After`, or 429,
is a rate-limit outcome. A 404 is inaccessible or unavailable, not proof
of deletion. Preserve operation uncertainty when a response is lost or a
server failure follows a sent mutation.

## Validation status

The locked build, complete fixture run, all four core tests, workspace
clippy with all targets and features, the core's `wasm32v1-none` build with
all features, and formatting check passed on Linux with
`nightly-2026-11-15` on 2026-11-25. No result below is a live GitHub
observation.

| Controlled case | Observed result |
|---|---|
| HTTPS credential callback | A Basic challenge calls the supplied callback. The fixture accepts the token, ingests the pack and publishes the exact revision. |
| HTTPS create and fast-forward | Both update the actual disposable bare repository and report acceptance for the requested ref. |
| Stale expected remote state | Negotiation observes the actual old OID and refuses the stale request. The fixture receives no additional POST. |
| Incorrect HTTPS credential | Authentication fails within the callback attempt bound. |
| TLS hostname mismatch | Trusted-host controls authenticate before and after the isolated negative request. The mismatched host fails with `ErrorCode::Eof` and `ErrorClass::Ssl`, with zero credential callbacks, negotiations or push bodies and an unchanged remote ref. |
| Server ref policy refusal | The server receives the POST and emits `ng`. `Remote::push` returns `Ok(())`; `push_update_reference` contains the rejection reason. The wrapper refuses success and the remote ref stays unchanged. |
| Competing update after advertisement | The server changes its ref before processing the command, refuses the stale old OID through `ng`, and retains the competing ref. Transport success still cannot establish publication. |
| Personal-token API errors | Typed SDK errors retain authentication, permission and rate-limit responses. Required Checks access refused by the fixture remains unavailable. |
| Installation expiry and cache | A second token acquisition refreshes a cached expired token. A subsequent acquisition reuses the valid token. Both mint requests retain the configured repository scope. |
| Installation 401 | A read with a refused cached token triggers one refresh and one successful replay with the new token. The repository scope stays unchanged. |
| Installation token for Git | A token returned by the refresh preflight authenticates a successful HTTPS Git push. |
| Typed Checks response | The App client decodes a controlled successful List check runs response. This does not prove a live private repository grants access. |
| Mutation replay policy | After installation-token preflight, the token-authenticated writer sends exactly one request for each controlled 401, 503 and connection closed after receiving the body. |
| SSH agent | An Ed25519 identity in the disposable Unix-domain agent authenticates against the restricted SSH fixture. A configured username also works when the URL omits it. |
| SSH trust and unavailable agent | A different pinned host key refuses the connection. An empty agent and a missing agent socket both refuse authentication after the credential callback is reached. |

The first runner attempt exceeded the Unix socket path limit under the
deep experiment directory. Scratch now uses the checkout's shorter
`target/tp.*` directory and socket name `a`. Initial HTTP fixture failures
exposed a missing TLS close notification, unhandled chunked request bodies,
and clean EOF before headers when a client refused the certificate. The
fixture handles these cases. Its threads own their counters and send
observations and stop requests through channels. The final run passed with
all listeners and fixture processes removed afterward. The TLS negative
case records the actual SDK error; it does not require a stable
certificate-specific error code.

## Unsupported and untested configurations

No live GitHub repository, organization App installation, private Checks API,
SSO policy or token revocation was exercised. The API fixtures use plain
HTTP on loopback. They establish request and response handling, not the
permission grants of a real token or Octocrab's live TLS connection.
App JWT issuance uses a disposable RSA key; the fixture checks the request
shape and repository scope, not GitHub's verification of its signature.

The SSH fixture verifies authentication, username selection, host trust and
the receive-pack handshake. SSH pack transfer and ref publication remain
unverified. `0v2csrkb` must verify them before enabling SSH publication.

Authenticated clone/fetch pack negotiation, expiry during a Git transfer,
refresh after a Git authentication rejection, proxies, redirects, credential
origin changes, custom enterprise authorities, multi-ref publication, Git
LFS, submodules, filters, alternate ref backends, SHA-256 repositories and
platform differences need their own evidence before being supported. The
fixture is not evidence for arbitrary server hooks or GitHub rulesets. It
establishes the protocol distinction between transport success and server
ref acceptance.

This decision does not resolve cancellation, object retention, external
checkout coordination or merge/filter support. Those remain assigned to
the other prerequisite decision and implementation tasks. Production
replacement still requires configured capability preflight and the
approved acceptance checks. Unsupported configurations must refuse before
dependent effects; the contract must not silently narrow to fixture-only
behavior.

## Work this decision unblocks

This evidence unblocks the transport prerequisite of
`0v2csjxn`, credential resolution and capability verification. The runtime
configuration prerequisite `0v2cshgk` remains. It supplies the verified
publication recipe for `0v2csrkb` in `vcs-git2` and the credential and
mutation retry policy for `github-api`. The latter supports the subsequent
issue reads in `0v2csk5t` and hosted observations in `0v2cssyv`. It does not itself complete those adapters or the parent
spec's production-replacement acceptance. `0v2cspkx` must verify the
selected authenticated clone/fetch path before switching home/project sync.
`0v2csjxn` and `0v2cssyv` must establish configured access, including any
required private Checks API access, before dependent operations run.
These checks gate activation. The implementation method is settled.
No contract narrowing is proposed.

## Primary sources

Checked against the pinned released source on 2026-11-25:

- [git2 credentials](https://docs.rs/crate/git2/0.21.0/source/src/cred.rs)
  defines direct user/password and SSH-agent credentials. The helper API
  is separate and is not used here.
- [git2 callbacks](https://docs.rs/crate/git2/0.21.0/source/src/remote_callbacks.rs)
  defines negotiation, certificate checking and ref-status callbacks.
- [git2 push update](https://docs.rs/crate/git2/0.21.0/source/src/push_update.rs)
  defines old and new OID accessors.
- [libgit2 push implementation](https://github.com/libgit2/libgit2/blob/v1.9.7/src/libgit2/push.c)
  calls negotiation before queuing objects and writes old/new update OIDs.
- [Git pack protocol](https://git-scm.com/docs/pack-protocol)
  specifies receive-pack advertisement, old/new ref commands and
  `unpack`/`ok`/`ng` status reports.
- [Octocrab client](https://docs.rs/crate/octocrab/0.54.9/source/src/lib.rs)
  defines repository-scoped installations, buffered token acquisition,
  cached expiry, token-authenticated clients and the installation 401 path.
- [Octocrab retry policy](https://docs.rs/crate/octocrab/0.54.9/source/src/service/middleware/retry.rs)
  defines `RetryConfig::None` and general retry behavior.
- [GitHub installation authentication](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/authenticating-as-a-github-app-installation)
  documents installation-token expiry, repository permissions and HTTP Git
  authentication with `x-access-token`.
- [GitHub fine-grained token limitations](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens#fine-grained-personal-access-tokens-limitations)
  and [check runs](https://docs.github.com/en/rest/checks/runs#list-check-runs-for-a-git-reference)
  document credential-kind and Checks API restrictions.
- [GitHub REST troubleshooting](https://docs.github.com/en/rest/using-the-rest-api/troubleshooting-the-rest-api)
  distinguishes authentication, access and rate-limit responses.
