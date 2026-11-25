# Backend implementation tasks

Spec `0v265ymj` was signed off by the owner on 2026-11-25 at Conductor
commit `fcc220a9`. The [approved contract](project-backend-contracts.md)
and [crate structure](project-backend-structure.md) define the work. This
slice creates two decisions, 21 implementation tasks and one design review.
All issues are in the home repository on `github-backends`.

## Standards and execution

The [standards project](https://github.com/JimLundin/standards) is the
implementation and validation authority. Every issue links it and the
approved contract. The reviewed baseline is `45c7290e`; workers use the
current declared profiles and the project's AGENTS.md.

Each implementation task covers a narrow path through the layers it needs
and ends at an agreed observable seam. The two prefactors establish domain
contracts while the existing paths still work. Caller batches then move to
the new backends. The retirement task removes the remaining old paths
only after every caller batch is integrated. Temporary coexistence is
internal to this branch, with one domain model and no compatibility API,
dual write, migration or runtime fallback.

The issue prefactor keeps the temporary Markdown codec in
`home::issue::storage` and `home::issue::document`. The CLI and daemon use
that codec at the `home-files` boundary. Task `0v2cswmj` removes it when
all issue callers use the tracker. `home::issue::body` remains the shared
interpreter for human Markdown sections, checklists and notes.

The leader uses the build skill on `github-backends`. Each worker gets its
own branch and worktree from that branch. Keep one target directory per
checkout and serialize builds/checks on this VM. After each merge, run
`STANDARDS=/path/to/standards tools/check.sh commit`; CI runs `step`.
Coordinate shared schema and manifest changes at integration rather than
adding artificial dependencies between unrelated behaviors.

The two decisions settle remaining implementation feasibility and policy,
not the already settled library choice. Only tasks that need their answers
are blocked. An answer that would narrow the approved contract must be
raised as a design change rather than silently weakening acceptance.

## Ordered issue list

1. [Verify project credential and transport capabilities](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2crv9w.md) (decision, `0v2crv9w`).

    Which supported credential and transport configurations can the first adapters verify without Git or gh subprocesses, including HTTPS token refresh, SSH-agent credentials, private GitHub checks and ref-level server rejection?

    Blocked by none.

2. [Verify snapshot installation and Git recovery limits](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cse85.md) (decision, `0v2cse85`).

    What concrete repository-feature, object-retention, cancellation and recovery policy makes snapshot installation and local session integration safe with git2 before replacing the existing backend?

    Blocked by none.

3. [Make issue flow operations independent of Markdown storage](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csfhh.md) (task, `0v2csfhh`).

    Refactor the existing home issue and flow path to accept domain issues and return semantic changes. Keep Issue, the flow interpreter, role defaults, graph validation and checklist behavior as the single definitions. Move file decoding/encoding to the existing storage boundary while callers are migrated; do not introduce a second issue model.

    Blocked by none.

4. [Extract repository operations into the vcs core](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csftz.md) (task, `0v2csftz`).

    Extract repository facts, revision/ref requests, preservation decisions and integration outcomes into the pure vcs crate. Keep session lifecycle in host and issue branch policy in home. Convert the current shell calls at the existing workspace and repository seams without changing the backend implementation in this prefactor.

    Blocked by none.

5. [Carry qualified issue identities and invalid observations](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csgbw.md) (task, `0v2csgbw`).

    Replace locally minted identity assumptions with a tracker authority and opaque issue key, separate from display locators and creation ordering. Carry valid, invalid, unavailable and deleted observations, with available human content, findings and freshness. Update existing consumers and generated client types in the same change.

    Blocked by [0v2csfhh](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csfhh.md).

6. [Configure project backends and expose their capabilities](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cshgk.md) (task, `0v2cshgk`).

    Add independent version-control, tracker and hosting configuration with host credential references. Introduce project-runtime's effectful interfaces over the existing home and vcs domain values, and a shared capability/preflight query available to CLI and daemon construction.

    Blocked by [0v2csgbw](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csgbw.md), [0v2csftz](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csftz.md).

7. [Persist and resume operations on their owning host](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csjdj.md) (task, `0v2csjdj`).

    Implement owner-bound operation handles, typed progress and durable records in project-runtime. Bind the owner before first dispatch, persist original intent/preconditions and each confirmed effect, and expose operation status/reconciliation through the shared service.

    Blocked by [0v2cshgk](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cshgk.md).

8. [Resolve project credentials and verify adapter capabilities](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csjxn.md) (task, `0v2csjxn`).

    Wire host credential references into one Octocrab client source and git2 callbacks using the verified configurations from the transport decision. Reuse Octocrab's personal-token/App installation support and token refresh. Add typed capability and access observation for the selected repository.

    Blocked by [0v2cshgk](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cshgk.md), [0v2crv9w](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2crv9w.md).

9. [Read GitHub issues into the shared cache and CLI](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csk5t.md) (task, `0v2csk5t`).

    Implement the GitHub field/body mapping and typed Octocrab list/read path, through project-runtime to CLI issue observations. Parse one project-data TOML footer, native title/body/creation time, managed kind/stage/status/priority labels and ordinary labels. Exclude pull requests.

    Blocked by [0v2csjxn](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csjxn.md), [0v2csjdj](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csjdj.md).

10. [Read GitHub relations and notes into flow validation](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csm0e.md) (task, `0v2csm0e`).

    Extend the shared read path through native parent/dependency endpoints, metadata relations and comment pages. Each relation uses exactly its configured authority. Preserve arbitrary cross-project identities and the existing home graph's kind/cardinality/cycle rules.

    Blocked by [0v2csk5t](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csk5t.md).

11. [Apply online semantic issue edits with recoverable progress](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csm16.md) (task, `0v2csm16`).

    Route existing-issue field, title/body, checklist, stage/status and relation edits from CLI through home validation and project-runtime into github-api. Preserve ordinary labels, human prose and the footer's other values. Use add/remove endpoints for managed labels and explicit parent moves.

    Blocked by [0v2csm0e](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csm0e.md).

12. [Create issues and append notes without blind retries](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cspaa.md) (task, `0v2cspaa`).

    Implement online issue creation and append-note requests through CLI and the shared operation service. Include adapter operation markers, store returned remote identities, and distinguish confirmed creation from unknown response. Use explicit steps for any requested initial relationships.

    Blocked by [0v2csm0e](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csm0e.md).

13. [Use git2 for repository discovery and home synchronization](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cspkx.md) (task, `0v2cspkx`).

    Implement typed discover/open/clone, refs and fetch operations in vcs-git2 and use them for project checkout discovery plus home configuration/instruction synchronization in CLI and daemon. Keep issue refresh results separate from home Git sync.

    Blocked by [0v2csjxn](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csjxn.md).

14. [Create owned workspaces and capture or install snapshots with git2](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csqna.md) (task, `0v2csqna`).

    Implement workspace creation/removal and snapshot capture/compare/installation through VersionControl and vcs-git2 under the recovery policy decided by the workspace decision. Preserve source checkout contents and ownership; keep capability refusals before dependent effects.

    Blocked by [0v2cshgk](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cshgk.md), [0v2cse85](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cse85.md).

15. [Integrate session work with recoverable Git refs](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csqwd.md) (task, `0v2csqwd`).

    Implement the vcs/project-runtime local integration path using git2 merges, expected ref state and persisted intent/receipt protocol. Retain candidate objects under the workspace decision's policy. Keep issue-flow transitions out of this adapter.

    Blocked by [0v2csqna](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csqna.md).

16. [Publish an exact revision through the Git backend](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csrkb.md) (task, `0v2csrkb`).

    Implement home project publish through a typed operation plan, owner-bound progress and vcs-git2. Create or fast-forward the requested ref using the verified expected-remote negotiation contract and inspect ref-level server acceptance.

    Blocked by [0v2cspkx](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cspkx.md), [0v2csjdj](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csjdj.md).

17. [Observe hosted checks and correlate queue attempts](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cssyv.md) (task, `0v2cssyv`).

    Implement RepositoryHosting through pure github mapping and typed Octocrab status/check/workflow requests. Observe exact revisions and configured queue attempts with their baseline/run identities. Add only configured workflow dispatch requests required by project operations.

    Blocked by [0v2csjxn](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csjxn.md).

18. [Run declared project checks against an exact revision](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csstk.md) (task, `0v2csstk`).

    Implement home project check and its client result through the shared service. Resolve the project's declared standards profiles and registered standards checkout, prepare an owned checkout for the exact revision, and run only that declared check entry point. Combine local and hosted observations without losing their revision or scope.

    Blocked by [0v2csqna](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csqna.md), [0v2cssyv](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cssyv.md), [0v2csjdj](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csjdj.md).

19. [Start issue work only after verified project preparation](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cst3g.md) (task, `0v2cst3g`).

    Connect the approved Start work plan to the existing host/daemon lifecycle and CLI client-link path. Validate the actual proposed active transition, prepare a safe workspace and confirm the issue update before launching. Persist the operation/session association before the launch effect.

    Blocked by [0v2csm16](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csm16.md), [0v2csqna](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csqna.md), [0v2cspkx](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cspkx.md).

20. [Finish sessions through integration and recoverable issue updates](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cstwa.md) (task, `0v2cstwa`).

    Migrate session finish to the Git integration/publication service and the online issue summary/status path. Keep the summary turn and session lifecycle in host. Integrate into the issue branch, publish when configured, append the summary and move to review-role status only under the existing added-commits rule.

    Blocked by [0v2cst3g](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cst3g.md), [0v2csqwd](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csqwd.md), [0v2csrkb](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csrkb.md), [0v2cspaa](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cspaa.md).

21. [Use GitHub issue observations and online edits in the app](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cstw1.md) (task, `0v2cstw1`).

    Move the daemon/app issue-list and editing path to project-runtime, with generated schemas for observations and operation results. Show invalid records, cache freshness/completeness, direct-edit conflicts, capabilities and recoverable partial/unknown writes.

    Blocked by [0v2csm16](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csm16.md), [0v2cspaa](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cspaa.md).

22. [Land reviewed project work through the configured queue](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csv9r.md) (task, `0v2csv9r`).

    Implement home project land and its client outcome through the approved flow transition, revision-bound approval, operation store, queue publication and hosting observation. Use a queue ref unique to the operation and record its baseline evidence.

    Blocked by [0v2csrkb](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csrkb.md), [0v2csm16](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csm16.md), [0v2cspaa](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cspaa.md), [0v2csstk](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csstk.md).

23. [Remove replaced issue storage and Git subprocess paths](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cswmj.md) (task, `0v2cswmj`).

    Delete the old Markdown issue storage, offline commit/replay machinery, file-shaped issue API and application Git/gh subprocess translations after every caller batch has moved. Keep home configuration/instruction files and the standards/CI tooling that still legitimately use Git.

    Blocked by [0v2cstwa](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cstwa.md), [0v2cstw1](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cstw1.md), [0v2csv9r](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csv9r.md).

24. [Review backend module depth and operation boundaries](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cswh4.md) (review, `0v2cswh4`).

    Review the complete spec branch against main after the caller migrations and retirement task. Apply the deepen skill to the changed modules, with focus=design. This is the one design review required by the slice; code/acceptance review at the review stage remains separate.

    Blocked by [0v2cswmj](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cswmj.md).

## Acceptance coverage

Numbers refer to the signed-off parent spec. Decision evidence informs the
listed implementation tasks; a prototype does not count as shipping their
behavior.

| Parent acceptance | Implementation owners |
|---|---|
| 1. Storage-independent issue/flow behavior | [0v2csfhh](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csfhh.md), [0v2csgbw](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csgbw.md), [0v2cshgk](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cshgk.md), [0v2csm0e](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csm0e.md), [0v2csm16](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csm16.md), [0v2cst3g](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cst3g.md), [0v2cswmj](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cswmj.md) |
| 2. GitHub field, relation and direct-edit mapping | [0v2csfhh](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csfhh.md), [0v2csk5t](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csk5t.md), [0v2csm0e](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csm0e.md), [0v2csm16](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csm16.md), [0v2cspaa](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cspaa.md), [0v2cstw1](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cstw1.md) |
| 3. Qualified identity and branch ownership | [0v2csgbw](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csgbw.md), [0v2csk5t](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csk5t.md) |
| 4. Cache completeness and no offline delivery | [0v2cshgk](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cshgk.md), [0v2csjdj](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csjdj.md), [0v2csk5t](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csk5t.md), [0v2csm0e](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csm0e.md), [0v2cstw1](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cstw1.md), [0v2cswmj](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cswmj.md) |
| 5. Semantic writes, operation ownership and uncertain recovery | [0v2csjdj](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csjdj.md), [0v2csm16](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csm16.md), [0v2cspaa](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cspaa.md), [0v2cstwa](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cstwa.md), [0v2cstw1](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cstw1.md), [0v2csv9r](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csv9r.md) |
| 6. Host start/finish behavior | [0v2csjdj](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csjdj.md), [0v2cst3g](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cst3g.md), [0v2cstwa](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cstwa.md) |
| 7. Landing transition and approval validation | [0v2cst3g](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cst3g.md), [0v2csv9r](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csv9r.md) |
| 8. Queue/check/deployment evidence and recovery | [0v2csjdj](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csjdj.md), [0v2csrkb](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csrkb.md), [0v2cssyv](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cssyv.md), [0v2csstk](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csstk.md), [0v2csv9r](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csv9r.md) |
| 9. Git workspaces, integration and publication | [0v2csftz](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csftz.md), [0v2cspkx](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cspkx.md), [0v2csqna](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csqna.md), [0v2csqwd](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csqwd.md), [0v2csrkb](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csrkb.md), [0v2cstwa](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cstwa.md), [0v2cswmj](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cswmj.md) |
| 10. Typed GitHub HTTP boundary handling | [0v2csjxn](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csjxn.md), [0v2csk5t](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csk5t.md), [0v2csm0e](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csm0e.md), [0v2csm16](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csm16.md), [0v2cspaa](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cspaa.md), [0v2cssyv](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cssyv.md) |
| 11. Credentials, transport and repository-feature limits | [0v2cshgk](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cshgk.md), [0v2csjxn](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csjxn.md), [0v2cspkx](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cspkx.md), [0v2csqna](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csqna.md), [0v2csqwd](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csqwd.md), [0v2csrkb](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csrkb.md), [0v2cssyv](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cssyv.md), [0v2csstk](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csstk.md), [0v2cswmj](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cswmj.md) |
| 12. Client schemas and app outcomes | [0v2csgbw](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csgbw.md), [0v2cshgk](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cshgk.md), [0v2csjdj](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csjdj.md), [0v2csstk](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csstk.md), [0v2cst3g](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cst3g.md), [0v2cstwa](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cstwa.md), [0v2cstw1](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cstw1.md), [0v2csv9r](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csv9r.md), [0v2cswmj](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cswmj.md) |
| 13. Shared standards and integrated validation | Every implementation task; final integrated gate in [0v2cswmj](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cswmj.md) |

## Initial frontier

- [0v2crv9w](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2crv9w.md): Verify project credential and transport capabilities.
- [0v2cse85](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cse85.md): Verify snapshot installation and Git recovery limits.
- [0v2csfhh](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csfhh.md): Make issue flow operations independent of Markdown storage.
- [0v2csftz](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2csftz.md): Extract repository operations into the vcs core.

The design review [0v2cswh4](https://github.com/JimLundin/home/blob/github-backends/projects/hyn162/issues/0v2cswh4.md) waits for the retirement task. It examines the completed
branch's module depth, ownership and test seams. The later review stage
still supplies separate standards/code and spec-acceptance verdicts. The
spec remains off main until its review and landing process completes.
