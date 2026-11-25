# GitHub issue storage and project operations

Research and proposed design, 2026-11-25. Tracked by Conductor spec
`0v265ymj`, at `shape`, on branch `github-backends`. This records a proposal,
not an approved implementation plan.

## What this decides

Use an existing Rust GitHub client behind Conductor's issue and repository
hosting interfaces. Keep the flow's decisions in Conductor. Agents should
request project operations whose implementation performs the required Git
and GitHub actions.

Git manages local repositories, commits and worktrees. GitHub provides
issues, checks and workflow runs. They need separate interfaces even when
one project operation uses both.

The owner chose to replace local Git subprocesses with a Rust library and
to support direct creation and editing of issues on GitHub. The flow can
be simplified, but must remain independent of GitHub. Issue changes require
connectivity; cached issues remain readable offline.

## Library findings

Primary sources checked on 2026-11-25. These recommendations are based on
documentation and published source, without a compilation or live API test.

| Library | Finding | Recommendation |
|---|---|---|
| [Octocrab 0.54.9](https://crates.io/crates/octocrab/0.54.9) | Typed REST operations, including issue relationships, checks and Actions. Published 2026-10-31. | Use for GitHub. |
| [Octorust 0.10.0](https://docs.rs/crate/octorust/0.10.0/source/) | Generated from GitHub's OpenAPI description. Published 2025-03-12. The released source lacks sub-issue and issue-dependency endpoints. | More custom endpoint code for this use case. |
| [git2 0.21.0](https://docs.rs/git2/0.21.0/git2/) | Rust bindings to libgit2, including worktrees, indexes, merges, rebases, fetch and push. | Preferred local Git candidate, subject to the landing constraint below. |
| [gix 0.91.0](https://crates.io/crates/gix/0.91.0) | Pure Rust. The project's [status report](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md) still lists incomplete push support. | Does not cover Conductor's full publication path today. |

Octocrab's [issue handler](https://docs.rs/octocrab/0.54.9/octocrab/issues/struct.IssueHandler.html)
provides issue creation, reading, listing, updates, comments and labels.
It also provides native sub-issue and blocked-by dependency operations.
Its [repository handler](https://docs.rs/octocrab/0.54.9/octocrab/repos/struct.RepoHandler.html)
provides refs and commit statuses. The
[checks](https://docs.rs/octocrab/0.54.9/octocrab/checks/struct.ChecksHandler.html),
[workflows](https://docs.rs/octocrab/0.54.9/octocrab/workflows/struct.WorkflowsHandler.html)
and [Actions](https://docs.rs/octocrab/0.54.9/octocrab/actions/struct.ActionsHandler.html)
handlers cover check results, workflow runs and dispatch.

The [builder](https://docs.rs/octocrab/0.54.9/octocrab/struct.OctocrabBuilder.html)
supports personal tokens and GitHub Apps. The
[client](https://docs.rs/octocrab/0.54.9/octocrab/struct.Octocrab.html)
provides installation clients, token refresh and pagination helpers.
Its GraphQL API accepts serialized queries and typed responses; it does not
generate query-specific types. The proposed initial GitHub operations have
REST coverage.

One local Git constraint needs a prototype before committing to the full
replacement. [git2's Transaction](https://docs.rs/git2/0.21.0/git2/struct.Transaction.html)
documents that committing several reference updates is not atomic and can
partially succeed. Conductor's `LandRefs` currently updates the issue branch
and landing receipt through `git update-ref`. A replacement must preserve
recoverable landing, including interruption between ref writes. Choosing
git2 alone does not establish that behavior.

[Repository](https://docs.rs/git2/0.21.0/git2/struct.Repository.html)
provides conditional single-ref updates through `reference_matching` and
in-memory merges through `merge_commits`. Its `set_index` and the
[index API](https://docs.rs/git2/0.21.0/git2/struct.Index.html) cover scratch
snapshot construction. The prototype must verify source-index preservation
and conflict handling against Conductor's required behavior.

[Remote::push](https://docs.rs/git2/0.21.0/git2/struct.Remote.html#method.push)
returning success does not establish that every ref was accepted. The
adapter must inspect `push_update_reference` results. Git2 is synchronous;
blocking workers and operation-specific cancellation must replace the
current subprocess execution policy.

The [git2 manifest](https://docs.rs/crate/git2/0.21.0/source/Cargo.toml)
has no default features, so select HTTPS or SSH support explicitly.
[Credential helpers](https://docs.rs/git2/0.21.0/git2/struct.Cred.html#method.credential_helper)
run configured processes and can retain a dependency on `gh`. Prefer direct
credential callbacks for a backend that does not depend on the GitHub CLI.
SSH agent support exists, but full compatibility with OpenSSH configuration
was not confirmed. Conductor's current daemon wrapper disables hooks and
fsmonitor; standards checks should remain explicit project operations.

## What exists

These observations refer to Conductor commit `423fe3d0`.

- [The issue model](../crates/home/src/issue/model.rs) already has domain
  operations for fields, status moves, notes and checklists. Its kinds,
  stages and rules come from the home's `flow.toml`.
- [Snapshot](../crates/home/src/flow/snapshot.rs) and
  [graph reading](../crates/home/src/flow/graph.rs) still consume issue
  files. [Command results](../crates/home/src/flow/command.rs) produce
  complete file writes. Replacing filesystem calls alone would keep
  Markdown storage inside the domain interface.
- [Issue delivery](../crates/home/src/issue/operation.rs) records edits,
  creates commits and retries delivery through Git. Both
  [the CLI](../crates/home-cli/src/main.rs) and
  [the daemon](../crates/daemon/src/home.rs) must use the new issue backend.
- [Workspace plans](../crates/host/src/workspace/plan.rs) already describe
  worktree, snapshot and landing operations. The
  [daemon](../crates/daemon/src/workspace.rs) runs their Git commands.
  [Architecture decision D7](architecture.md) chose Git's executable to
  retain its worktree, hook and configuration behavior.
- [The landing queue](../.github/workflows/queue.yml) consumes `queue/*`
  branches and reports a `queue` commit status. It is a custom Actions
  workflow. The project's [instructions](../AGENTS.md) prohibit opening
  pull requests.
- [Domain ownership](adr/0003-domains-own-their-types-and-the-link-composes-them.md)
  requires domain types to stay in their owning crates. GitHub SDK types
  should be converted at the backend boundary.

## Proposed interfaces

The issue backend reads domain issues and applies semantic changes such as
creating an issue, appending a note, updating fields or changing a relation.
It returns the stored identity and an explicit outcome. File paths, Markdown
front matter and Git commits are not part of that interface.

The local Git backend owns worktrees, snapshots, diffs, branch publication
and local merges. It will use a Rust library, as the owner requested.
The git2 candidate brings a C dependency through libgit2. It must also
replace Git subprocesses in home synchronization and repository discovery.
This includes the command syntax in
[checkout probing](../crates/home/src/checkout.rs) and the generic Git helper
used by [home-files](../crates/home-files/src/lib.rs) and
[the local CLI](../crates/home-cli/src/local.rs). Replace these with typed
observations through the same local Git adapter.

The repository hosting backend reads checks, commit statuses and workflow
runs, and performs the remote actions project operations need. It can share
one GitHub client and credential implementation with the issue backend.
There is no need to wrap every SDK endpoint in a second API.

The pure core describes operations and interprets their results as data.
An I/O trait or SDK implementation belongs in a shell crate. The CLI and
app should call the same operation service.

For example, a proposed `land` operation would verify the flow's review and
sign-off requirements, record the requested commit, publish the queue ref,
and observe the queue status for that commit. Only a confirmed landing
would complete the issue. The operation would retain enough progress to
resume after a host restart. It would distinguish queued, checking, failed
and landed outcomes. These are proposed behaviors, not commands available
today.

## Issue representation

GitHub would be authoritative for issues in a configured GitHub repository.
The home would continue to hold project configuration, shared instructions
and the flow. A local cache would not be another independently writable
issue store.

Prefer GitHub's title, body, labels, comments, sub-issues and dependency
relations where their meaning matches the flow. A note can use a comment;
human comments must not be interpreted as approved stage transitions.
Native relations can represent `parent` and `blocked_by` where GitHub's
limits permit them. GitHub allows
[100 sub-issues per parent and eight levels](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/adding-sub-issues).
[Adding a sub-issue](https://docs.github.com/en/rest/issues/sub-issues#add-sub-issue)
requires the same repository owner and uses a global numeric issue id for
the child, distinct from the parent's repository-local issue number.
[Issue dependencies](https://docs.github.com/en/rest/issues/issue-dependencies)
have their own REST operations. Cross-owner parents and arbitrary future
flow relations need a declared representation in the adapter. GitHub's
limits must not become limits of Conductor's flow. Where native relations
cannot represent it, the adapter needs another encoding. This mapping
remains to be designed.

The flow has more states than GitHub's open and closed issue states.
GitHub's native issue `status` is
[preview-only and ignored when unavailable](https://docs.github.com/en/rest/issues/issues#update-an-issue).
Namespaced labels are the proposed mapping for kind, stage, status and
priority. Branch names and other arbitrary fields need one structured
representation. Choose that representation before implementation; duplicating
each field across labels, body metadata and a local file would create
conflicting values.

[Native issue types](https://docs.github.com/en/issues/tracking-your-work-with-issues/configuring-issues/managing-issue-types-in-an-organization)
now support user-owned repositories too. Octocrab's
[create](https://docs.rs/octocrab/0.54.9/octocrab/issues/struct.CreateIssueBuilder.html)
and [update](https://docs.rs/octocrab/0.54.9/octocrab/issues/struct.UpdateIssueBuilder.html)
builders omit `type`, so using it would need a small typed request through
the same client. The final field mapping remains open.

The [repository issues endpoint](https://docs.github.com/en/rest/issues/issues#list-repository-issues)
also returns pull requests. Filter entries with `pull_request` set.

The current issue id is global across projects. GitHub issue numbers are
repository-scoped. The backend must retain GitHub's stable identity and its
repository and number for display and API addressing. The domain needs a
provider-qualified identity or a deliberate mapping; `#42` alone cannot
identify a cross-project dependency.

This proposal replaces storage for configured projects without a migration
or dual writes. The project's existing-data policy applies.

## Reliability decisions to settle

- A successful remote write must be distinguishable from a queued request
  and a request whose response was lost. Retrying issue or comment creation
  without reconciliation can create duplicates.
- GitHub updates to an issue, its comment and its relationships are separate
  requests. Project operations need recorded progress and recovery for
  partial success.
- Direct GitHub edits can violate the flow. Conductor should report those
  issues and refuse affected operations. It cannot prevent a GitHub user
  from making the edits.
- Cached reads need freshness and completeness information. Missing access,
  an incomplete page or an unavailable repository must not look like an
  empty issue list or a resolved dependency.
- Issue writes require connectivity. Cached issues remain readable offline,
  with their freshness visible. Remove the offline edit/replay path. Recovery
  records for requests that might already have succeeded are still needed.
- Polling fits the current hosts without opening inbound ports. The polling
  policy must respect GitHub's rate limits.

GitHub's [best practices](https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api)
document conditional reads with ETags. Neither that documentation nor the
issue update endpoint establishes an atomic `If-Match` write contract.
A read-before-write check cannot promise to prevent a race with a GitHub
UI edit. No cross-issue transaction was confirmed.

Octocrab's [retry configuration](https://docs.rs/octocrab/0.54.9/octocrab/service/middleware/retry/enum.RetryConfig.html)
includes rate-limit handling, but retries can also repeat server and
transport failures. The published implementation does not restrict retries
to idempotent methods. Disable blind retries for issue and comment creation;
reconcile an uncertain response before resubmitting. The SDK does not
provide exactly-once project operations.

## Open shaping questions

Settled with the owner: replace local Git commands with a Rust library,
allow direct GitHub issue edits, keep the flow independent of GitHub while
allowing simplification, and require connectivity for issue changes.

Further decisions include the first project operations to expose, storage
for arbitrary flow fields, behavior for local-only projects, authentication
and handling of concurrent remote edits. The current queue remains the
proposed landing implementation. Local landing recovery with git2 needs a
focused prototype before implementation, tracked by decision `0v26ayew`.
