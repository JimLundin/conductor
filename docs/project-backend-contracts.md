# Issue mapping and project operation contracts

Signed off by the owner for spec `0v265ymj` on 2026-11-25 at commit
`fcc220a9`. The approval covers the backend boundaries, issue mapping,
project operations, recovery and acceptance seams. Decision `0v26ayew`
selects git2 after the local prototype. These operations remain to be
implemented; the [task graph](project-backend-tasks.md) defines that work.

The [crate structure](project-backend-structure.md) owns the dependencies.
The [library research](github-backends.md) and
[prototype](experiments/vcs-probe/README.md) record the evidence. References
to existing code below describe Conductor at `ce97ebcd`.

## Standards

The [standards project](https://github.com/JimLundin/standards) is the source
of the implementation and validation rules. This contract was checked against
its [README at 45c7290e](https://github.com/JimLundin/standards/blob/45c7290e8ee1b3d6ae1b5bde860a889454e6469c/README.md).
Follow its current declared profiles when building; this reference records
the design review baseline, not a fork of those rules.

- Apply the [Rust layers and data-only core API](https://github.com/JimLundin/standards/blob/45c7290e8ee1b3d6ae1b5bde860a889454e6469c/README.md#rust).
  `home`, `vcs` and `github` hold pure decisions and build with all features
  for `wasm32v1-none`. Effectful interfaces belong in `project-runtime`.
  Adapters declare the shell layer. Reuse SDK models at their boundary and
  pass declared domain values to the cores.
- Apply the standards' thin-shell analysis to the new adapters and to the
  merged project. Splitting a conditional into another shell function does
  not move its decision into the core.
- Follow the [TypeScript standard](https://github.com/JimLundin/standards/blob/45c7290e8ee1b3d6ae1b5bde860a889454e6469c/README.md#typescript)
  for app changes, including pure `web/core` decisions and `@effect/vitest`.
  Generate client schemas through Conductor's existing tools and bump its
  client ALPN when the reachable contract changes.
- Use the [standard's runner and tiers](https://github.com/JimLundin/standards/blob/45c7290e8ee1b3d6ae1b5bde860a889454e6469c/README.md#running-the-checks).
  Run affected format, lint and focused behavior checks while building,
  then `STANDARDS=/path/to/standards tools/check.sh commit`. CI runs `step`;
  `release` remains the standard's release gate. Do not enable local higher
  tiers or weaken checks through the project operation.
- Declare additional suites in Conductor's `checks.toml` using the
  [standards' suite contract](https://github.com/JimLundin/standards/blob/45c7290e8ee1b3d6ae1b5bde860a889454e6469c/README.md#suites).
  No new suppression, compatibility reader or duplicate test runner is
  part of this work. Apply Conductor's existing-data policy in place.

## First implementation

Implement git2 version control and Octocrab tracking and hosting. Keep
`VersionControl`, `IssueTracker` and `RepositoryHosting` independent.
GitHub is the first tracker, not a prerequisite of a project or session.

Projects without a tracker retain directory and project sessions, local
snapshots and diffs. They have no issue list and refuse
issue-specific operations with `TrackerNotConfigured`. A local project can
also use a separately configured GitHub tracker. No second tracker or
Markdown issue fallback is included in this implementation.

Home configuration, instructions, skills and standards profiles stay in
the home repository. Replace the application's Git subprocesses for that
repository's discovery, clone and synchronization with the same Git
adapter. Home configuration sync and issue refresh have separate results.
Project build tools and existing CI workflows still run their declared
commands; this does not rewrite the standards project's own Git tooling.

## Domain records and identity

Keep `home::issue::Issue`, its typed fields, named relations, ordered
checklist and flow interpreter. Replace file-shaped inputs and complete
file writes with domain observations and semantic changes. Reuse the
existing body-section and checklist interpretation independently of issue
front matter. The adapter strips its metadata before those helpers run.

Exclude pull requests from tracker lists and direct issue reads before
mapping them. GitHub's Issues API returns them too; Octocrab's
`Issue.pull_request` identifies those records.

An issue ID is an opaque string unique within its project's tracker, and a
project and an ID name one issue. The GitHub adapter gives each issue its
number as its ID. The issue's REST ID and GraphQL node ID are addresses in
its locator. The core never parses an ID. A relation names an issue of its
own project by its ID, and an issue of another project by that project and
the ID. A command reads it as `<project> <id>`, such as `hyn162 0trfecpc`,
where `<project>` is the project's name or ID. A stored relation keeps the
two apart, as `{ id = "0trfecpc", project = "hyn162" }`.
Sort issues by their creation time with identity as a deterministic tie
break, not by the old locally minted ID.

Resolve redirects before refreshing a locator, and check the returned
identity and configured repository. A moved issue with changed identity or
an unconfigured destination requires explicit reconciliation. Do not
promise identity preservation across every provider transfer. A `404` is
unavailable or inaccessible, not proof of deletion. Confirmed deletion and
missing access have different outcomes.

An explicit branch field continues to win. Otherwise `home` derives a
Git-valid branch from a bounded stable digest of the full qualified identity,
rather than a repository-local number. Persist the chosen branch and full
identity before creating it, and refuse a collision with another recorded
owner. Branch spelling alone is not proof of ownership. Locator refresh
never renames an existing branch. The Git backend validates the resulting ref.

The read result retains each record's identity, locator, available human
title/body, observation time and one of valid, invalid, unavailable or deleted.
Unavailable records retain last-known text when there is any. Invalid
metadata must not make an issue disappear. A valid issue also carries flow
findings. The app shows those findings beside the issue.

## GitHub mapping

Each configured field has one authoritative source. The
configuration declares defaults and label mappings against the loaded
flow. It rejects duplicate mappings and undeclared values before writes.
The default managed-label namespace is `work/`; projects can select a
different unused namespace. No project brand appears in the format.

| Domain value | Authoritative GitHub representation |
|---|---|
| Title | Native title. |
| Body, sections and checklist | Human issue body, including the existing `## Checklist` convention, outside the adapter footer. |
| Creation time | Native `created_at`, assigned to the field with the flow's created role. |
| Kind | One mapped label, such as `work/kind/task`; without one, the flow's default kind. |
| Stage | One mapped label, such as `work/stage/build`; no label means no stage. |
| Status | One mapped label, such as `work/status/in-progress`; without one, the configured default for native open, completed or not-planned state. |
| Priority | One mapped label, such as `work/priority/high`; without one, the flow's declared default. |
| Ordinary labels | Native labels outside the reserved namespace, assigned by the flow's labels role. |
| Other typed fields, including branch | One typed TOML footer in the issue body. |
| Notes | Native comments, with provider comment identity, author and creation time. |
| Relations | Native sub-issues and dependencies, and footer entries, as described below. |

Do not use preview issue status or native issue types in the first adapter.
The explicit label map can represent the flow without depending on those
APIs. Unknown reserved labels, multiple values for one field, bad metadata,
or an inconsistent native state produce visible mapping findings.

A flow status with a terminal role projects to closed/completed; a dropped
role projects to closed/not-planned; other roles project to open. When a
status label exists, it remains authoritative and the native open/closed
state must agree. Closing or reopening a labelled issue in GitHub without changing
its status label therefore produces a conflict to resolve, not a guessed
flow transition. Without a status label, ordinary GitHub create, close and
reopen use the configured defaults. Missing `state_reason` has an explicit
configured closed default; unsupported reasons without a label produce a
finding.

With an explicit status label, native state reason is advisory. Send the
corresponding reason when closing an open issue. GitHub can ignore a reason
update when state does not change. A terminal-to-dropped move on an already
closed issue therefore changes the authoritative label and reports a stale
native reason as a warning. It does not reopen and close the issue just to
change that projection. This distinction preserves the flow's status
changes without claiming unsupported GitHub behavior.

The adapter uses add/remove-label endpoints for labels it owns. It does
not replace unrelated human labels. A label transition can take several
requests and temporarily be invalid. Store progress and verify the final
observation before reporting success or starting a dependent operation.

### Additional data and relations

The footer is a single final fenced block with info string `project-data`,
containing TOML. It is visible and editable in GitHub. For example:

````text
```project-data
schema = 1

[fields]
branch = "backend-interfaces"
method = "prototype"
```
````

The footer has declared typed fields for its schema, optional operation
marker, additional domain fields and metadata-backed relations. Missing
footer means empty additional data. Duplicate footers, unknown schema or
duplicated native fields are invalid. The body editor changes human text
and preserves the footer; field and relation commands change only their
mapped entries. A create writes its operation handle to the optional
top-level `operation` string as `<owner-host>/<operation-id>` and keeps
it for reconciliation.
The `[relations]` table holds a qualified issue identity for a single target
and an array of qualified identities for multiple targets.

The backend chooses how it stores each relation, and projects configure
no relation sources. GitHub holds `parent` as a sub-issue and `blocked_by`
as an issue dependency. The footer holds any other relation, and any edge
GitHub can't hold, such as one to a Markdown issue or to another tracker.
A read takes `parent` and `blocked_by` from both the native structures and
the footer as one relation. A single-target relation that names two
different issues across the two forms is invalid.

GitHub allows up to 100 sub-issues per parent, eight levels of nesting and
50 issues for each dependency type. A sub-issue must belong to the same
repository owner as its parent. A native edge to an issue in another
repository reads as a warning, and its relation's coverage stays
incomplete, because the backend can't name that issue's project. Native
limits are adapter capabilities, not flow limits. An unsupported native
edge is refused before effects where the limitation is known; a server
rejection remains an explicit outcome.

Read all relevant relation pages. An inaccessible dependency remains
unresolved and cannot satisfy a flow rule. A parent move is a distinct
semantic request; do not silently replace a parent while adding a child.
Graph cardinality, kind restrictions and cycle checks remain in `home`.

Human comments remain notes even when their text looks like a stage move
or approval. Operation comments have an adapter marker and a receipt
reference. They are audit output, not a source of approval. Append notes
in this version; Conductor commands for editing or deleting existing
comments are outside scope. Refresh still reflects those direct GitHub
edits and the completeness or access limits of its observation.

## Reads, edits and recovery

An observation contains the issue or invalid record, findings, a revision
fingerprint, and freshness and completeness for the fields and relations
used. That fingerprint is a comparison of observed values, not a GitHub
write precondition. Conditional GET results and `updated_at` alone do not
prove an unchanged relation graph.

Refresh uses polling, pagination and conditional reads. Publish a new
complete list only after all pages succeeded. Retain the previous complete
snapshot on failure and expose the attempted refresh's failure separately.
Track completeness for comments and relationships independently. A list
can be fully paged without being a transactional snapshot. Fetch an issue
and the facts a rule needs again before acting.

Cached issues remain readable offline with their last successful refresh.
An offline mutation is refused before it becomes a pending issue edit.
Remove `PendingEdits`, the offline replay path and the home issue commit
count. Operation records remain for requests already sent or prepared as
part of a multi-step operation. A prepared but unsent issue write is not
automatically scheduled for delivery after connectivity returns.

Edits carry an operation ID, the observed revision and a semantic change.
Reuse the existing checklist's seen-list check. The runtime records the
original intent, owner host, preconditions and per-step results before
effects. The operation handle binds the owner host before first dispatch,
so a missing record on another host cannot look like a new operation. The
same ID with different input is refused. CLI and daemon use the same host
store namespace, and a repository operation holds its workspace's lock. An
issue operation claims its issue, or for an issue it opens its ID, with an
interprocess lock held until its attempt ends, so writes to one issue on
one host never overlap, writes to other issues go ahead, and a second
write to a claimed issue is refused as in progress. A note rerun after a
lost reply resumes the unsettled note on the same issue with the same
words, so it finds its marker and posts nothing more. A retry on another
host must reach the owner or report it unavailable; it must not start the
operation again.

GitHub does not support conditional issue PATCH. Re-read before writing,
refuse a known changed precondition, send only the affected fields or
label/edge changes, then read back. Do not automatically merge body edits
or retry after an observed conflict. Preserve the submitted intent for
reconciliation. Independent hosts and GitHub users can still race within
the read/write window. A successful read-back proves the observed result,
not the absence of a lost concurrent edit.

Create-issue and append-note requests include an operation marker in their
bodies. Record returned provider identities. Disable blind SDK retries for
non-idempotent requests. After a lost reply, reconcile the known identity
or enumerate raw issue/comment bodies rather than trusting search indexing.
One matching marker and intent can establish the result. No match is not
proof that creation failed; duplicate matches, changed markers, incomplete
reads and missing access require reconciliation. Do not automatically
resubmit an uncertain create or note, even after a completed empty scan.

Relation and label changes also keep per-step progress. On retry, compare
observed fields against the recorded expected intermediate state, not just
the original complete fingerprint. The operation can resume its own
partially applied labels or state even when they form an invalid mapping.
Validate the proposed final issue and unchanged external preconditions,
and refuse unrelated changes or uncertain attribution.
Never roll back a partial result by overwriting newer human work. The
runtime exposes the confirmed changes and what remains uncertain.

Backend write results distinguish verified application, refusal before
effects, partial application, and unknown outcome. They include the
operation identity and available recovery action. Transport acceptance is
not completion. Generic HTTP success or an SDK retry setting cannot choose
a flow transition.

## Project operations

The commands below are semantic contracts; `home-cli` and the app use the
same service. Proposed CLI names are `home work start`, `home project publish`,
`home project check` and `home project land`. Preserve today's `home check`
for issue/flow validation and issue-edit commands where their meaning
survives. Final argument spelling can be settled in the CLI task.
An agent cannot pass arbitrary Git arguments, API routes or shell commands
through a backend method.

| Operation | Inputs and preflight | Confirmed result |
|---|---|---|
| Start work | Project, issue, checkout and observation; valid flow and graph, no conflicting local assignment, ready branch and safe workspace snapshot. | The project service confirms preparation and the active issue update; the host then launches or returns the recorded session. |
| Publish | Project, branch, exact revision and expected remote ref state; a configured remote and supported transport. | The remote contains the revision at the requested ref, with ref-level rejection checked. |
| Check | Project, exact revision and declared check scope; resolve the project's standards profiles and hosting requirements. | Local check result and hosted statuses or runs each retain their revision, scope and completion state. |
| Land | Issue branch, exact reviewed revision, current flow facts and explicit approval for that revision; supported configured queue. | Queue acceptance, checking, failure or confirmed landing, followed separately by the issue update. |

`Start work` strengthens today's behavior. `project-runtime` prepares the
workspace and issue progress; it has no dependency on `host`. The existing
host/daemon path coordinates that preparation with session launch. CLI
start requests reach this path through the client link and require a
reachable host service. Other project operations can use the shared local
service directly.

Validate the proposed active transition before launching the agent and
wait for the required issue write to be confirmed. The host persists the
operation-to-session association before launch and uses its durable session
history to reconcile a lost launch reply. If the workspace is prepared but a later step fails,
the operation retains it for retry or explicit cleanup. A retry reuses the
workspace and session identity. An active issue update followed by launch
failure remains an explicit partial start. It does not erase a changed
workspace or roll an issue back over a newer edit. Cross-host independent starts still
require conflict observation; a local assignment check is not a distributed
lease.

Session finish remains a separate host operation. It captures the final
snapshot, integrates the session into its issue branch, publishes when
configured, and appends the summary. Preserve the current move to the
review-role status only when integration added commits. If issue completion
fails after integration, expose that partial result and retry the issue
step without integrating again. The [git2 protocol](experiments/vcs-probe/README.md)
governs local branch and receipt recovery.

`Publish` creates or fast-forwards its target. It does not force-push over
a competing update. The adapter checks the expected remote state during
push negotiation as well as ref acceptance, with a focused test required
before this is a production guarantee. After an uncertain push it reads
the ref and objects before deciding whether anything can be retried.

`Check` runs only the project's declared standards entry point and profile
at the requested revision, in an owned checkout whose result can be tied
to that revision. A dirty working copy is not evidence about a committed
revision. Local default is `commit`; CI supplies `step` and `release` under
the standards policy. Hosted checks are observations of the same revision.
An unavailable required Checks API capability is a refusal, never a pass.
The app distinguishes pending, failed, passed, unavailable and stale results.

`Land` first plans a move from the observed issue into the project's
configured landing stage. Evaluate the actual before/after transition,
including sign-off and transition-only rules such as finished children,
before enqueueing anything. Record that plan and confirm its stage update
before queue publication. An issue already at the landing stage without
this operation's verified entry record cannot start a fresh land attempt;
it needs an explicit flow move back and a newly validated entry. Resume
uses its recorded entry and checks for unrelated changes.

`Land` keeps the current queue-ref and commit-status process. Use a unique
`refs/heads/queue/<operation-id>` ref for the attempt. Pin the submitted
revision, queue ref and baseline status/run identities before publication.
Match the `queue` status to that submitted commit and the
operation's queue attempt, not to the current branch tip. A success carries
the resulting default-branch revision, which can differ after the queue's
rebase. Verify it against the configured queue run and repository before
closing the issue. Older successful statuses are not a new attempt's receipt.

If the queue deletes its ref or the publication reply is lost, reconcile
queue status and workflow evidence before republishing. If landing is
confirmed but the issue update fails, report landed with issue update
pending. A retry cannot enqueue it again. Queue failures do not complete the
issue. Changes the existing queue refuses, including workflow changes,
return that refusal; the runtime does not silently push to `main`.

Landing and deployment are separate results. Do not infer a successful
deployment from a queue status or a successful dispatch. When deployment
is required by project policy, match the deployment run to the landed
revision and wait for its result before completing the issue. Validate the
proposed terminal status transition separately against fresh flow facts;
a newly broken rule leaves the code landed and the issue update refused.
The stale Pages revision recorded in bug
`0v2am3gb` remains separate corrective work; the new contract must expose
that mismatch rather than report deployment success.

Approval for `Land` is an explicit caller assertion bound to the issue,
reviewed revision and a fingerprint of relevant flow/configuration facts.
The fingerprint excludes observation timestamps. Changing those facts or
the reviewed revision requires a new approval. The fingerprint also names
the approved plan. Its own recorded stage and issue changes are expected
progress, not outside changes that invalidate approval. A completed operation
still returns its recorded outcome without asking for approval again. This adds revision binding
to today's `signed_off` boolean; it does not introduce a new identity or
cryptographic approval system. Direct GitHub edits can change current state
and violate flow rules. They never start work or supply an approval by
themselves. Snapshot validation does not claim to reconstruct unseen
transition history.

## Backend methods and outcomes

Keep requests and results in their owning domain. The shell interfaces
provide these bounded capabilities:

The owner clarified during implementation that `VersionControl` models
project work, not the Git command model. Its callers request owned
workspaces, snapshots, comparisons, session integration and publication of
an exact revision. Git indexes, object construction, ref transactions and
worktree mechanics stay inside the implementation. Typed Git commands may
exist during the extraction, but must not become the shared backend port.

| Interface | Required requests |
|---|---|
| `IssueTracker` | Read an issue; list issues with continuation/completeness; read notes and relations; create; apply a validated semantic edit; append a note; observe a prior operation. |
| `VersionControl` | Locate or obtain a project repository; observe revisions and workspace ownership; prepare/remove an owned workspace; capture/compare/install snapshots; integrate against an expected target revision with a recoverable result; synchronize; publish an exact revision; observe publication. |
| `RepositoryHosting` | Read declared commit statuses, check runs and workflow runs for a revision; observe a configured queue attempt; dispatch only a configured workflow when an operation requires it. |

Capabilities belong to the configured repository and credential, not just
the adapter's name. Refusals distinguish unsupported capability, unavailable
credentials, missing access, conflict, invalid issue, incomplete observation
and failed precondition. Errors after effects include the operation record.
SDK types, Markdown front matter and raw JSON do not enter the core API.

## Credentials and transport

Project configuration names a repository and a credential reference.
Enrolled hosts resolve a `fleet/` reference from the signed, encrypted
fleet configuration and check the repository binding before use. A
standalone host resolves that reference from its private store. The fleet
root publishes an entry from its private store and republishes it after
rotation. See [fleet administration](fleet.md).

Octocrab authenticates with OAuth tokens, personal access tokens and
GitHub App installations. It refreshes installation tokens. Git2 HTTPS
callbacks obtain credentials from the same resolver. SSH uses the host's
agent with a pinned server key. The adapters perform operations in-process.

A fine-grained personal token supports the current queue through Issues,
Contents, Commit statuses and Actions permissions. It does not provide
private Checks API access in the current documented contract. Use an App
installation with the relevant permissions when required checks need that
API. Missing permissions and expired credentials are explicit outcomes.
No capability check is itself permission to create a live issue or push a
branch during validation.

## Acceptance seams

Use the smallest seam that exercises Conductor's decision. Adapt existing
tests before adding another test harness. No end-to-end suite is proposed
without an observed bug that needs one.

| Seam | Scenario and expected result |
|---|---|
| `home` issue/flow core | The same semantic edit from CLI and app yields the same plan, findings and refusal without reading an issue file. |
| `home` graph core | Cross-project edges, arbitrary declared relations, kind/cardinality violations and cycles retain their flow meaning independently of GitHub limits. |
| `home` checklist core | A changed seen-list refuses an indexed edit; no different checklist entry is changed. |
| `github` mapping core | A directly created native issue has defaults without a normalization write; unknown or conflicting managed values remain visible as invalid records. |
| `github` mapping core | Direct body/checklist/label/relation edits appear on refresh; comments that resemble approval stay ordinary notes. |
| `github` mapping core | Close/reopen with a contradictory explicit status label reports the conflict; unlabelled close/reopen maps through configured role defaults. |
| `github` mapping core | One footer round-trips arbitrary typed fields and metadata relations while human prose and unrelated labels survive. A relation read from both native structures and the footer is one relation. |
| `home` identity and branch core | Equal issue numbers in two repositories remain distinct; a locator change cannot change an existing branch; unavailable transfer identity is reported. |
| `project-runtime` with scripted adapters | Offline edits have no remotely deliverable pending item; failed pagination retains a stale complete cache and an incomplete refresh result. |
| `project-runtime` with scripted adapters | A stale observation refuses before effects; a partial label/edge update records confirmed steps; unknown body/create/note outcomes are reconciled without blind resubmission. |
| `project-runtime` with scripted adapters | Reusing an operation ID with different input is refused; a retry after restart returns or resumes the stored result; another host cannot silently take over the same attempt. |
| Existing host start/finish planners | An invalid active transition prevents agent launch; finish integration remains distinct from queue landing; a failed summary/status update remains recoverable. |
| `home` project operation core | A changed reviewed revision or planning observation invalidates the supplied approval. An issue label or comment cannot approve landing. |
| `project-runtime` with scripted adapters | Lost queue replies, deleted queue refs, old statuses and successful landing followed by an issue failure never cause an unverified success or duplicate enqueue. |
| `github` hosting mapping | Queue success retains submitted and landed revisions; a deployment for another revision cannot satisfy a required deployment. |
| `vcs-git2` disposable repositories | Preserve the prototype's worktree ownership, source index, snapshots, conflicts, conditional refs and interrupted local integration; add snapshot installation and competing publish cases. |
| `github-api` controlled HTTP boundary | Exercise request mapping, pagination, rate limits, disabled unsafe retries, permission errors and per-step recovery with typed SDK inputs/results. Test our handling, not Octocrab itself. |
| Configured credential/transport validation | Verify the selected HTTPS/SSH path, credential refresh, ref-level server refusal, required merge/filter behavior, cancellation and object retention before replacing production Git operations. Record any unsupported configuration as a preflight refusal. |
| Existing client schemas and web core | Invalid records, cache freshness, capability refusals and partial results reach the app through generated schemas with the new client ALPN. |
| Standards runner | Changed crates and dependents pass the declared Rust/TypeScript profiles, the integrated `commit` gate passes locally, and CI `step` passes under the standards project. |

## API evidence for the mapping

Checked on 2026-11-25 against GitHub documentation and Octocrab 0.54.9.

- [Issue update](https://docs.github.com/en/rest/issues/issues#update-an-issue)
  defines state and state reason, including when the reason is ignored.
- [Conditional requests](https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api#use-conditional-requests)
  explicitly exclude unsafe methods unless an endpoint says otherwise.
- [Get issue](https://docs.github.com/en/rest/issues/issues#get-an-issue)
  distinguishes redirects, unavailable destinations and deletion.
  [Transfers](https://docs.github.com/en/issues/tracking-your-work-with-issues/administering-issues/transferring-an-issue-to-another-repository)
  can change addressing and retained labels.
- [Sub-issues](https://docs.github.com/en/rest/issues/sub-issues#add-sub-issue)
  and [dependencies](https://docs.github.com/en/rest/issues/issue-dependencies)
  have separate operations. [Native sub-issue limits](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/adding-sub-issues)
  do not constrain the core graph.
- [Octocrab's issue handler](https://docs.rs/octocrab/0.54.9/octocrab/issues/struct.IssueHandler.html)
  supplies typed issue, label and relation requests. Its create-with-sub-issues
  helper performs several writes; do not use it as an atomic hierarchy
  operation. Its published `update_comment` uses POST while GitHub documents
  PATCH; comment editing is not part of this contract.
- [Issue creation](https://docs.github.com/en/rest/issues/issues#create-an-issue)
  and [comment creation](https://docs.github.com/en/rest/issues/comments#create-an-issue-comment)
  document no idempotency key. Our operation markers are application data,
  not a provider exactly-once guarantee.
- [Personal tokens](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens),
  [check-run access](https://docs.github.com/en/rest/checks/runs#list-check-runs-for-a-git-reference)
  and [App installation authentication](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/authenticating-as-a-github-app-installation)
  define the credential capabilities and token lifetime.
