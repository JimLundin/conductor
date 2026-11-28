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
credential = "fleet/github-issues"
options = '''
namespace = "work/"
'''

[backends.hosting]
adapter = "github"
resource = "team/builds"
credential = "fleet/github-builds"
```

`credential` names the credential that hosts use for this repository.
Credential references cannot traverse directories, and backend resources
reject URLs with embedded credentials or query strings. `options` is TOML
text interpreted by the selected adapter. GitHub mapping options are
validated against the loaded flow by the pure `github` crate. Its defaults do
not apply to other adapters.

Run `home project capabilities <project>` from the home checkout to
validate the configuration and inspect the available operations. This
reads the project catalog without refreshing issue records. Tracker mapping
validation also reads the issue flow. Local and hosting capability inspection
works when that flow is missing or invalid and no tracker needs it.
Malformed project configuration is still found and reported.
Invalid configuration fails the query. A successful query can report
unavailable operations, including missing backends, missing or expired
credentials, and unsupported capabilities.

The CLI registers the local workspace adapter and the GitHub tracker when
it can construct them. The tracker can always read issues; it can also
write issue comments when its credential declares `write_issues`. Use the access command below to check the
credential against the remote repository.

`project-runtime::Registry` provides the same registration, capability and
preflight query for CLI and daemon construction. A binding includes its
complete selection, resource, credential reference and options. Changing
any of these requires a new binding. Capability checks run before adapter
dispatch and apply to the requested operation. A tracker failure does not
prevent local workspace preparation, snapshots or diffs. Issue work without
a tracker returns `TrackerNotConfigured`.

A comment write, `Project::write_comment`, runs as an operation in the
host's operation store. Its body must carry its handle's marker,
`Handle::marker`. The write re-reads the issue's comments and is refused
when they differ from what its caller saw. It then sends one request and
reads the comments back. It confirms the write only once it finds the
marker. When the reply is lost, the outcome is unknown and stays recorded.
Retrying the same handle looks for the marker again and never sends the
request twice. A request the tracker rejects leaves the operation ready to
send again.

A relation write, `Project::write_relation`, sets one relation of an issue
to a list of targets, as an operation in the same store. The tracker
re-reads the issue and plans the steps that take the relation there. The
runtime refuses the write when the issue's revision differs from what its
caller saw. It sends one step at a time and re-reads the issue after each.
It confirms a step once the re-read no longer needs it. When a reply is
lost, the step stays awaiting evidence, and a retry re-reads the issue
instead of sending again.

GitHub's tracker keeps `parent` as a sub-issue and `blocked_by` as a
dependency when the target is an issue of the same repository. A target in
another project goes to the footer, as does any other relation. A parent
move sends `replace_parent`. The tracker refuses a write that would give a
parent more than 100 sub-issues, nest sub-issues more than eight levels
deep, or give an issue more than 50 blockers, or a blocker more than 50
issues it blocks. It sends nothing for a refused write. GitHub's 422 reply
leaves the operation ready, as an invalid write.

An issue edit, `Project::edit_issue`, such as `home issue set my-project 42
parent=40`, sends its label, title, body and state writes first. It then
hands each relation it sets to `Project::write_relation`, as an operation of
its own, so GitHub keeps the relation natively where it can. A new issue's
relations follow its create the same way. A GitHub issue's revision covers
its title, body, labels and state, not its native relations: a relation
write reads those afresh before each step.

Before it plans a write, the CLI takes each project's issues from its
tracker's listing: the home's files for a Markdown project, and the cached
listing for a tracker. A relation target is checked only against a listing
that is complete, so a target in a tracker that was never listed, or whose
last listing failed, is left unchecked.

Repository hosting declares statuses, checks and workflows separately.
Access to statuses does not imply access to required checks. Hosted-result
requests retain their exact revision. Workspace methods use `vcs` values
for preparation, capture, comparison, integration and preservation; Git
commands, indexes and object operations do not cross this interface.

## Reading tracked issues

Run these commands from the home checkout. The project argument accepts
a name or project ID. The issue argument is the issue's ID, which for a
GitHub issue is its number.

```sh
home issue list my-project
home issue show my-project 42
home issue show my-project 42 --offline
```

Both commands print JSON. A refresh reads direct GitHub edits. The list
includes open, closed and invalid issues. Each record includes its ID,
locator, findings and coverage. `home issue show` also reads the issue's
comments, so its log and notes are complete. The list doesn't read
comments, so its records have incomplete notes coverage.

A successful refresh reports `Fresh`. If a page fails, the command exits
with status 1 and reports the failure alongside the previous complete list.
`--offline` reads the host cache and reports `Stale` with the observation
time. Use `--data-dir <dir>` to select the host data directory.

`home issue list` and `home issue show` also read a project without a
tracker, from its Markdown issues, and print the same JSON. Every other command that names an issue, such as
`home canvas put my-project 42 <name>`, takes the same `<project> <id>`
and looks the ID up in the cache first. If the cache lacks it, the command
refreshes the project's tracker once and looks again. An ID still missing
fails with `no issue 42 in my-project`. If the refresh fails, or
`home issue show` was given `--offline`, the command fails and names
`home issue list my-project`.

The CLI refuses to write a second Markdown record for a project with a tracker.
A write to a Markdown issue or its canvas commits the files it wrote to the
home, authored by `HOME_BY`, through the git2 backend.

## Opening issues and adding notes on GitHub

`home issue new my-project task "Title"` on a GitHub project sends one
create through `Project::open_issue`. The issue carries its kind label, a
stage label when it has a stage, and a status label unless its status is
the open default. Its body holds the kind's sections and a project-data
footer whose `operation` field is the create's operation handle. The
command prints the number GitHub gave the issue, as `wrote my-project/147`.

The create runs as an operation in the host's operation store. Before
sending, it lists every issue of the repository and settles on the one
whose footer names its handle and whose title matches. It confirms the
create the same way after sending. When the reply is lost, the outcome is
unknown and stays recorded. Running the same `issue new` again resumes
that operation, by its project, title and labels, and looks for the
issue again. It never sends a second create, even after a list that finds
no issue.

Every log entry a Markdown write would add becomes one comment on the
GitHub issue, through `Project::append_note`: a note, a move with its
reason or sign-off, a set, and the `opened.` entry of a new issue. A
comment opens with a hidden `conductor-log` marker holding its operation
marker. The entry follows in the Markdown log's format without its
bullet, signed by `HOME_BY`. A read of the issue turns these comments into its log. A
comment without the marker is a person's note, even when its text reads
like a move. A canvas comment is neither.

## Canvases on GitHub

GitHub keeps each canvas of an issue as one comment on it, through
`github_api::Canvases`. The comment opens with a hidden marker holding the
canvas's name, its signer from `HOME_BY` and the write's operation marker.
The canvas follows as Markdown with Mermaid, as `home canvas show` prints
it, and then its data in a collapsed `<details>` block. The data block is
the canvas: an edit to the Markdown on GitHub is lost at the next put. A
canvas's time is its comment's `updated_at`.

`home canvas put` writes through `Project::write_comment`: it edits the
comment of that name, or creates one, and fails before any request when the
comment would pass GitHub's limit of 65,536 characters. A put needs GitHub
reachable and a credential with `write_issues`. The store keeps a create's
operation handle under the canvas's key until GitHub shows the comment: a
create whose result is unknown is settled by its marker on the next put,
and while GitHub's list doesn't show it yet the put fails rather than create
a second comment.

A refresh of the project, such as `home issue list`, ends with the store's
own refresh, which reads the repository's marked comments on the listed
issues into GitHub's canvas cache, `github-canvases.redb` in the host data
directory. `home canvas show` and the app read them from there offline. A
marked comment whose name reads but whose data or signer doesn't is that
canvas, broken, and the next put of its name edits it. A marked comment
whose name doesn't parse is a stray at the comment's URL, and so is a
second comment holding the same canvas; `home check` reports each.

## Repository access and credentials

Run `home project access <project> [host-data-dir]` to check which
GitHub endpoints the configured credentials can read. It looks up the
repository and reads one page of issues, statuses, checks or workflow
runs according to the selected backends. The corresponding host command,
`orchestrator project access <project> [--data-dir <dir>]`, uses the
home checkout synchronized by that host. It reports each access failure
and exits unsuccessfully if any probe fails. The capability query above
reports which project operations the installed adapters can execute.

Enrolled hosts resolve `fleet/` references from their latest verified
fleet configuration. The root shares each entry with the enrolled hosts
in a signed, encrypted configuration. A missing or withdrawn entry makes
access unavailable. Each entry binds one credential to one repository.

A standalone host reads the same reference from its private credential
store. The root uses that store to provision the shared entry. On Unix
the store is `project-credentials.toml` under the host data directory.
The process user must own the file and restrict its permissions to that
user. A fine-grained token entry has this form.

```toml
["fleet/github-issues"]
repository = "team/issues"

["fleet/github-issues".source]
kind = "fine_grained"
token = "<token>"
```

After provisioning or rotating the root entry, publish it with
`orchestrator fleet --data-dir <dir> credential share fleet/github-issues`.
See the [fleet instructions](fleet.md) for sharing and withdrawal.

An optional `expires_unix` on the entry declares its expiry in Unix
seconds. Add `write_issues = true` to let Conductor create and edit issue
comments with the credential. Without it, a comment write is refused
before any request is sent. Use `kind = "classic"` for a classic personal token or
`kind = "oauth"` for an OAuth token. A GitHub App installation uses
`kind = "installation"` with numeric `app_id` and `installation_id` and
an RSA PEM `private_key`. Git over HTTPS acquires its token from the
same credential. A Git SSH credential uses `kind = "ssh_agent"` with
the pinned server key blob in base64 as `host_key`. It uses the agent
with the `git` user on `github.com`. On Windows, the same entry lives in
Windows Credential Manager under the service `Conductor project credential`.
The account is the canonical host data directory, a vertical bar and the
credential reference. Its secret is the entry's TOML with a `[source]` table.
