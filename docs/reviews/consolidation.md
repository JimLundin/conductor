# Inactive branch consolidation

Assessment began at `90cbb89` and incorporates the landing-queue work on
`main` through `2513fd7`, 22 November 2026. The historical
reviews describe older versions of Conductor; their proposals do not
override the current glossary or the accepted product decisions.

## Architecture reviews

| Source | Current disposition |
| --- | --- |
| `architecture-review-20261121` (`bf81c7b`) | Returning-session import and movement orchestration are obsolete because sessions stay on one host. Shared configuration acceptance remains unfinished on its implementation branch. Shared key custody and moving fleet form decisions into the UI core remain design candidates. |
| `architecture-feature-cuts-20261122` (`e6815b8`) | Session-local checklists, removal of task handoff and automatic checklist notes, host-bound sessions, and removal of historical undo are already implemented. |
| `architecture-scope-review-20261122` (`67cbb16`) | Commit snapshots retain turn diffs and ordinary Continue. Removing Finish, issue editing, detailed child transcripts or between-turn effort changes was proposed, not accepted. |
| `architecture-cohesion-review-20261122` (`7ada611`) | Release-fixed CLI pins have replaced fleet version/settings publication. Removing app enrollment, project cloning, Finish, issue editing or effort changes remains a product decision. |

The retained product still supports phone issue editing, automatic Finish,
project cloning, app enrollment, child transcripts and requests, and
between-turn effort changes. Consolidating review history removes none of
those behaviors. Original reports remain accessible through their commits.

The key-custody proposal must follow the current disposable-data policy:
one reader for each current format, with no legacy decoding or migration.
Fleet workflow decisions still belong in the existing pure UI core if that
candidate is taken forward; browser key access, storage and transport stay
in the shell.

## Rust cleanup

`rust-cleanup` (`211d3ad`) predates the current issue model, commit snapshots
and child request handling. Its applicable changes are integrated in the
current code:

- Borrow question drafts and credential form descriptions; retain nonsecret
  credential draft values in place.
- Route recorded commands with `host::log::Kind`, eliminating the second
  `Ledger` vocabulary and separate log entry points for its two kinds.
- Encode Codex calls directly, without a second set of parameter records.
  Current calls and omission of absent optional fields are preserved.
- Run browser clippy directly, including the UI core.

Streaming text already appends to owned string buffers. Actor requests
already share `daemon::ask`. Consuming a state value and returning its
replacement transfers ownership; it does not clone the state. The wider
mutable-update rewrite is therefore omitted. Current issue draft retention,
stale-start replies, child-scoped approvals, child metadata, duplicate tool
and summary handling, and commit-snapshot semantics remain authoritative.

## Headless salvage

`deepening-01-salvage` (`89885f3`) introduced a client-view-driven example.
Main already uses `HostFold` and `CompletedTurn`, builds the example from
`net`, and checks command acceptance before recording an answered request.
The remaining duplicate provider-choice enum is replaced by `Provider` and
its existing parser. No second turn type or historical undo path returns.

## Work that remains separate

`shared-configuration-application` (`3fc1de9`) is unfinished. Its acceptance,
credential reconciliation and retirement design remains relevant, but it
must be reconciled with credential-only fleet configuration and release-fixed
CLI pins before landing. It also needs application/retry UI, reliable
credential-refusal classification, one command-identity replay policy,
current-format-only persistence and provider retirement verification.
Its existing commit remains available; this consolidation does not claim
that design is implemented.

`ci-units` and `queue-land` were active when this assessment began. Their
completed CI and landing infrastructure subsequently reached `main` and
is retained without changing its implementation.
