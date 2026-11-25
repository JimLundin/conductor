# Project backend configuration

A project's `project.toml` selects version control, issue tracking and
repository hosting independently. Omitting a selection configures no
backend for that work. The project's existing `remote` still describes
where its checkout can be cloned; it does not select an issue tracker or
hosting service.

```toml
[backends.version_control]
adapter = "git2"
resource = "/projects/local"

[backends.tracker]
adapter = "github"
resource = "team/issues"
credential = "host/github-issues"
options = '''
namespace = "work/"
'''

[backends.hosting]
adapter = "github"
resource = "team/builds"
credential = "host/github-builds"
```

`credential` names a secret in host storage. It contains no token.
Credential references cannot traverse directories, and backend resources
reject URLs with embedded credentials or query strings. `options` is TOML
text interpreted by the selected adapter. GitHub mapping options are
validated against the loaded flow by the pure `github` crate. Its defaults
and native relation choices do not apply to other adapters.

Run `home project capabilities <project>` from the home checkout to
validate the configuration and inspect the available operations. This
reads the project catalog without refreshing issue records. Tracker mapping
validation also reads the issue flow. Local and hosting capability inspection
works when that flow is missing or invalid and no tracker needs it.
Malformed project configuration is still found and reported.
Invalid configuration fails the query. A successful query can report
unavailable operations, including missing backends, missing or expired
credentials, and unsupported capabilities.

The CLI registers the local workspace adapter and the GitHub issue reader
when it can construct them. Use the access command below to check the
credential against the remote repository.

`project-runtime::Registry` provides the same registration, capability and
preflight query for CLI and daemon construction. A binding includes its
complete selection, resource, credential reference and options. Changing
any of these requires a new binding. Capability checks run before adapter
dispatch and apply to the requested operation. A tracker failure does not
prevent local workspace preparation, snapshots or diffs. Issue work without
a tracker returns `TrackerNotConfigured`.

Repository hosting declares statuses, checks and workflows separately.
Access to statuses does not imply access to required checks. Hosted-result
requests retain their exact revision. Workspace methods use `vcs` values
for preparation, capture, comparison, integration and preservation; Git
commands, indexes and object operations do not cross this interface.

## Reading tracked issues

Run these commands from the home checkout. The project argument accepts
a name or project ID. Quote GitHub references so the shell passes the
number sign to the command.

```sh
home issue list my-project
home issue show my-project '#42'
home issue show my-project '#42' --offline
```

Both commands print JSON. A refresh reads direct GitHub edits. The list
includes open, closed and invalid issues. Each record includes its stable
ID, locator, findings and coverage. You can pass its stable ID to
`home issue show`. Comments and native relations have incomplete coverage
until their readers are available.

A successful refresh reports `Fresh`. If a page fails, the command exits
with status 1 and reports the failure alongside the previous complete list.
`--offline` reads the host cache and reports `Stale` with the observation
time. A first read with no cached result reports `Unknown` and exits
with status 1. Use `--data-dir <dir>` to select the host data directory.

Edit tracker-backed issues in GitHub during this read-only implementation.
The CLI refuses to write a second Markdown record for a project with a tracker.

## Repository access and credentials

Run `home project access <project> [host-data-dir]` to check which
GitHub endpoints the configured credentials can read. It looks up the
repository and reads one page of issues, statuses, checks or workflow
runs according to the selected backends. The corresponding host command,
`orchestrator project access <project> [--data-dir <dir>]`, uses the
home checkout synchronized by that host. It reports each access failure
and exits unsuccessfully if any probe fails. The capability query above
reports which project operations the installed adapters can execute.

On Unix the host reads `project-credentials.toml` in its data directory.
The process user must own this regular file and restrict its permissions
to that user. Each key names a project credential reference and binds
its credential to a single GitHub repository. For example, this
host-only file declares a fine-grained token.

```toml
["host/github-issues"]
repository = "team/issues"

["host/github-issues".source]
kind = "fine_grained"
token = "<token>"
```

An optional `expires_unix` on the entry declares its expiry in Unix
seconds. A classic token uses `kind = "classic"`. An installation uses
`kind = "installation"` with numeric `app_id` and `installation_id` and
an RSA PEM `private_key`. Git over HTTPS acquires its token from the
same credential. A Git SSH credential uses `kind = "ssh_agent"` with
the pinned server key blob in base64 as `host_key`. It uses the agent
with the `git` user on `github.com`. On Windows, the same entry lives in
Windows Credential Manager under the service `Conductor project credential`.
The account is the canonical host data directory, a vertical bar and the
credential reference. Its secret is the entry's TOML with a `[source]` table.
