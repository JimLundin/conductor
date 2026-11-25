# Project backend structure

Structure agreed by the owner for spec `0v265ymj` on 2026-11-25. Use git2
for the first version control implementation and Octocrab for GitHub task
tracking. Decision `0v26ayew` replaces the provisional gix choice because
gix 0.91.0 has no push execution API. The
[library research](github-backends.md) and
[local prototype](experiments/vcs-probe/README.md) record the evidence and
remaining checks. The interfaces keep both libraries outside the domain.

The [signed-off contract](project-backend-contracts.md) specifies issue
mapping, project operations, recovery outcomes and acceptance seams. The
[task graph](project-backend-tasks.md) assigns the implementation work.
It references the standards project as the source of implementation and
validation rules.

The owner selected `cli` as the binary crate and `orchestrator` as its
executable. Task `0v28ftxj` implemented that rename, which landed on main
at `777610b0`.

## Crates and responsibilities

Keep the current pure core and shell pattern. Extend `home` as the owner
of project work, extract version control from the session implementation,
and give both CLI and daemon one project operation service.

| Crate | Layer | Responsibility |
|---|---|---|
| `home` | Core, existing | Project catalog, issues, relations, flow rules, project operation plans and hosting results. |
| `vcs` | Core, new | Owned workspaces, snapshots, revisions, integration and publication; preservation rules for project work. |
| `project-runtime` | Shell, new | Backend interfaces, execution of project plans, operation records, cached reads and refresh. |
| `vcs-git2` | Shell, new | Implements the version control interface through git2 and libgit2. |
| `github` | Core, new | Pure GitHub field and relation mapping, including metadata GitHub cannot represent natively. |
| `github-api` | Shell, new | Implements tracking and hosting interfaces through one Octocrab client. |
| `home-files` | Shell, existing | Reads home configuration and instructions. Issue storage leaves this crate. |
| `home-cli` | Binary, existing | Offers the same project operations to terminal users and agents. |
| `host` and `daemon` | Core and shell, existing | Own session lifecycle and invoke project operations and version control through the shared service. |
| `cli` | Binary, existing | Builds `orchestrator`, constructs adapters, supplies credentials and injects the configured service. |

The module layout is:

```text
crates/
  home/src/
    project.rs
    issue/
    flow/
    operations/
    hosting.rs
  vcs/src/
    model.rs
    command.rs
    plan.rs
  project-runtime/src/
    backends.rs
    service.rs
    store.rs
  vcs-git2/src/
    repository.rs
    workspace.rs
    remote.rs
  github/src/
    mapping.rs
  github-api/src/
    client.rs
    tracking.rs
    hosting.rs
```

`home` continues to own the issue vocabulary, including specs, tasks,
decisions and bugs. There is no second task model alongside `Issue`.
The existing `runtime` crate remains the agent runtime.

## Interfaces

`project-runtime::backends` owns the effectful Rust interfaces. Requests,
results and refusals belong to the domain core that gives them meaning.
This keeps I/O traits out of the pure core's data-only API.

| Interface | Operations | Implementation |
|---|---|---|
| `VersionControl` | Locate project work, prepare and remove owned workspaces, capture and compare snapshots, integrate session work, synchronize and publish an exact revision. | `vcs-git2`, using git2. |
| `IssueTracker` | List and read issues, create issues, apply edits, append notes and change relations. | `github-api`, using Octocrab. |
| `RepositoryHosting` | Read checks, workflow runs and commit statuses; request supported hosted actions. | `github-api`, using the same Octocrab client. |

Checks and Actions belong to the hosting interface. A project operation
can use all three interfaces, while a tracker replacement only needs to
implement `IssueTracker`.

Methods accept domain values such as an issue edit or revision publication,
never arbitrary Git arguments, API paths or JSON. An operation that is
unsupported, partially applied or uncertain returns that outcome. A missing
capability is detected before a dependent operation changes state.

The owner clarified during implementation that this abstraction models
Conductor's intended work. Callers ask for a workspace, a preserved snapshot,
an integration result or publication of a reviewed revision. They do not
assemble Git operations. Index staging, object construction, ref updates,
worktree mechanics and Git recovery records belong inside the implementation.
An internal typed replacement for Git arguments is a refactoring step; it
is not the `VersionControl` interface. Tests of project plans use domain
outcomes and do not prescribe a sequence of Git commands.

The shell converts SDK types at the boundary. It reuses Octocrab's API
models and git2's repository types instead of defining another wire model.
The `github` core owns only the mapping facts and decisions it needs. It
does not mirror the GitHub API. This separate pure module keeps provider
encoding decisions out of both the flow and the I/O shell.

## Dependencies and construction

The domain dependencies point inward:

```text
home -> vcs
host -> home, vcs
github -> home
project-runtime -> home, vcs
vcs-git2 -> project-runtime, vcs, git2
github-api -> project-runtime, github, octocrab
daemon -> host, project-runtime
```

`project-runtime` does not depend on either concrete adapter. The `cli`
and `home-cli` binaries construct the adapters and pass them to the
service. Only the two adapter shells depend on git2 or Octocrab. `link`
continues to compose domain commands and results; SDK types never reach
the web app or an agent.

Backend selection is per project. Its version control, tracker and hosting
configuration are separate, so a Git repository does not require a GitHub
tracker. Configuration names the credential. The host resolves a shared
reference through its verified fleet configuration or, on a standalone host,
through its private store.

## A project operation

The project operation names are `start work`, `publish`, `check` and `land`.
These are operation names, not a committed CLI syntax. The app uses the
same service. Session lifecycle stays in `host`; project plans coordinate
issue and repository work and return the context for a session start.

For `land`, the core reads the issue and required review and sign-off
facts, then plans publication of the reviewed revision to the project's
queue ref. The runtime records the operation and executes publication
through `VersionControl`. It observes the existing queue's commit status
through `RepositoryHosting`. Once landing is confirmed, `IssueTracker`
records the result and the core chooses the appropriate flow transition.

If publication succeeds but the reply is lost, the service reads the
remote ref before retrying. If landing succeeds but updating the issue
fails, it retries that update without landing again. If the reviewed
revision changed, the core refuses to reuse the previous approval.
These recovery rules belong to the operation, not to an SDK retry setting.

Local integration has a separate recovery record. Before changing an
issue branch, persist the attempt identity, expected head, candidate,
result count and branch reflog baseline. Lock the branch and receipt while
observing and acting, and write one reference per step. A matching receipt
returns the recorded result. A branch containing the candidate permits
receipt repair without moving the branch. An unchanged expected head and
reflog permits publication. A rewritten branch or missing evidence requires
reconciliation. This protocol does not depend on an atomic multi-reference
transaction. The prototype checks reopened repository handles after each
completed write; power-loss durability remains unverified.

## Storage and direct edits

GitHub is authoritative for issues assigned to its tracker. A refresh
reads GitHub, applies the adapter mapping and validates the resulting
domain issues against the flow. Invalid direct edits remain visible with
their problems, and affected project operations are refused.

Cached issues remain readable offline. Issue edits require connectivity.
The service retains receipts for uncertain or partially completed remote
operations, but it does not accept a queue of offline edits. Current API
documentation does not establish atomic compare-and-set for issue updates;
the structure does not promise that a GitHub UI edit cannot race a write.

GitHub's native relations and fields are used where they fit. The mapping
for additional flow fields and relations is private to the GitHub adapter.
The contract specifies managed labels, one typed body footer and a
declared authoritative source for each relation. GitHub's limits do not
change the domain model.

## Moving the current code

1. Replace file-shaped issue inputs and writes in `home` with domain issue
   snapshots and changes. Keep its flow interpreter and rule evaluation.
2. Move workspace, snapshot, integration and preservation decisions out of
   `host::workspace` and the home repository helpers into `vcs`. Keep
   session policy in `host` and issue branch policy in `home`. Keep Git
   command planning inside the temporary executable implementation until
   the git2 implementation replaces it.
3. Move shared project I/O out of `daemon::home` and `home-cli::local` into
   `project-runtime`. Both callers receive the same outcomes.
4. Add the git2 and Octocrab implementations behind those contracts. Change
   the binary construction points and remove the replaced Git subprocess
   and Markdown issue paths.

Git2 is the first adapter choice, not a production implementation delivered
by the prototype. It adds libgit2 as a C dependency. Before replacing the
current backend, verify authenticated HTTPS/SSH, server-side push refusals,
the required merge and configuration behavior, cancellation, and coordination
with external checkout writers. Use credential callbacks without a Git or
GitHub CLI fallback. Keep uncertain outcomes explicit; the local recovery
probe does not establish universal exactly-once execution.
