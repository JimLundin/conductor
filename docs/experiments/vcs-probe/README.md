# Local Git adapter probe

Decisions `0v26ayew` and `0v2cse85`, spec `0v265ymj`. Verified on Linux with
`nightly-2026-11-15`, git2 `0.21.0` and libgit2 `1.9.7` on 2026-11-25.
This standalone workspace does not change Conductor's production
dependencies or select its production adapter.

Git2 implements the local repository operations exercised here without a
Git subprocess. Recoverable landing needs a stored intent and separate
reference writes. Git2 does not provide the atomic multi-reference update
used by Conductor's existing landing implementation.

The [installation policy](installation.md) extends this result with
retained candidate refs, snapshot installation, required attributes,
external checkout changes and cancellation limits. It defines the
supported repository scope and the evidence still required before
production replacement.

## Run

From Conductor's root:

```sh
cargo run --locked --manifest-path docs/experiments/vcs-probe/Cargo.toml
cargo test --locked --workspace --manifest-path docs/experiments/vcs-probe/Cargo.toml
cargo clippy --locked --workspace --all-targets --all-features --manifest-path docs/experiments/vcs-probe/Cargo.toml
cargo check --locked -p vcs-probe-core --all-features --target wasm32v1-none --manifest-path docs/experiments/vcs-probe/Cargo.toml
cargo fmt --all --check --manifest-path docs/experiments/vcs-probe/Cargo.toml
```

The executable creates disposable repositories under its own ignored
`target/probe/` directory and removes them on exit. It invokes no Git
executable and reads no credentials. Its remote is another directory.
HTTPS and SSH transport features are disabled.

Initially, three of the core's four recovery-policy checks failed with a
policy that always refused to resume. They reported the expected missing behavior.
The implemented policy passes those checks and the executable's filesystem
and reference scenarios.

## Observed results

| Operation | Result |
|---|---|
| Discovery | A nested directory in a linked worktree discovers that worktree and its common repository. |
| Worktree ownership | A wrong owner marker, another branch at the same commit, unexpected detached HEAD, or a dirty worktree refuses removal. |
| Worktree lifecycle | A linked worktree can be created, explicitly detached, and removed with its registration after checking the expected ownership. |
| Scratch snapshot | A dedicated repository handle with an in-memory index captures unstaged changes, untracked files, tracked deletions and ignored-but-tracked changes. It excludes ignored untracked files. |
| Source preservation | Snapshot capture leaves the source branch, HEAD bytes, real index bytes and worktree files unchanged. |
| Staging policy | The candidate cannot replace an index whose staged tree differs from both HEAD and the candidate. The staged content remains readable afterward. |
| Tree diff and merge | A disjoint merge retains both changes and both parents. Tree diff reports the expected added paths. A conflicting merge exposes the conflicting path and leaves the source branch unchanged. |
| Conditional reference | A stale expected OID refuses the update and preserves the competing writer's branch head. |
| Local transport | Initial push, clone, fast-forward push and fetch transfer the expected refs and objects. A divergent push returns `NotFastForward` and preserves the remote head. |
| Interrupted landing | Reopening after intent installation, branch publication and receipt installation completes with the stored count of two commits. Replays do not add another branch update. |
| Concurrent progress | A descendant of the candidate permits receipt repair without a rewind. An unrelated branch change refuses publication. |
| Rewritten progress | A published branch rewound before its receipt refuses recovery. A matching receipt remains authoritative after branch rewrite or deletion and an unrelated invalid HEAD. |
| Missing or inconsistent evidence | A missing reflog or wrong receipt refuses recovery. An existing intent cannot be replaced by another attempt's data. |
| Ref locking | A competing resume and a separate repository handle's conditional write cannot bypass held landing ref locks. |
| Checked-out branch | Landing refuses to publish an issue branch checked out in another worktree. |
| Snapshot installation | Reopened index/ref/receipt boundaries complete without changing source HEAD/index/files or session files. Clean and fully staged session indexes install; partial staging refuses. |
| Retained candidates | Owned refs make prepared snapshot and integration commits reachable. Missing retention refuses unfinished work. |
| Installation interference | External file/index/HEAD changes, a branch rewind and replacement checkout directory refuse continuation. Completed retries preserve later user edits. |
| Required attributes and modes | Capture and linked checkout preserve LF normalization, binary bytes, executable mode and file symlinks on Linux. A binary attribute refuses textual merging of disjoint edits. |
| Unsupported filters | A configured required external filter is skipped by capture. This mode requires production preflight refusal. |
| Cancellation and lock limits | A detached blocking worker completes its ref update. Stale Git locks remain untouched; branch/index locks do not prevent all HEAD/path changes. |

The push wrapper collects `push_update_reference` rejection messages as
well as the return value of `Remote::push`. The local non-fast-forward
scenario exercises a transport error. It does not exercise a server
returning `push()` success with a rejected reference. That distinction is
established by the published callback API, not by this run.

## Landing protocol

The prototype retains the candidate under an operation-owned ref and
stores one immutable JSON intent before changing the issue
branch. It contains the branch and receipt names, expected old OID,
candidate OID, added commit count and baseline reflog bytes. It installs
the intent without replacing an existing file and synchronizes that file
and its containing directory.

Each resume opens the repository and intent again. It locks the issue
branch and receipt refs, reads their state under those locks, and performs
at most one reference update:

1. A matching receipt returns the stored outcome without inspecting
   unrelated worktree HEADs or changing the issue branch.
2. A candidate or descendant branch with no receipt installs the receipt.
3. The expected branch with no receipt and unchanged reflog publishes the
   candidate, unless that branch is checked out.
4. Divergence, changed history at the expected OID, or an inconsistent
   receipt requires reconciliation.

The next resume repairs a missing receipt. The intent retains the original
count; recovery never recomputes it from a branch that another writer moved.
A transaction locks two refs but updates only one. This does not depend on
multi-reference atomicity. The baseline check occurs while the branch lock
is held, so a writer that honors Git's ref locks cannot advance and rewind
between that check and publication.

## Limits

This is a feasibility artifact, not a complete adapter. Snapshot capture
and installation are covered within the [declared scope](installation.md).
The fixtures use SHA-1 repositories on Linux. Submodules, Git LFS, sparse
indexes, alternate object stores, SHA-256 repositories and platform
differences require refusal until verified. External filters require
refusal; the probe demonstrates their missing behavior.

The interruption scenarios drop handles and reopen after each stored
boundary. They do not kill a process during a write or simulate power loss.
They do not establish fsync guarantees for libgit2's refs or objects,
cross-host coordination, stale-lock cleanup, or universal exactly-once
execution. Candidate refs establish reachability, not power-loss
durability or exclusion of concurrent external garbage collection.

Missing, pruned or changed reflog evidence causes refusal when recovery
would otherwise republish an expected branch. Deliberately restoring the
exact old reflog bytes or editing refs while bypassing their locks is
outside the protocol. A completed receipt still permits an outcome read
after a branch rewrite. Intent corruption, malicious edits and retention
cleanup need production handling.

Ref locks do not serialize arbitrary changes to worktree HEADs, files or
directories. The checked-out-branch observation and ownership checks are
verified without simultaneous filesystem edits. The extension demonstrates
HEAD/path changes despite held Git locks and refuses detected replacements.
Removal races, simultaneous checkouts and unusual ref backends remain
unproven. Installation requires a quiescent, exclusively controlled session.

No live GitHub request, credentials, HTTPS/SSH authentication, credential
refresh, hooks or server-side push rejection is tested. Blocking-worker
cancellation has local evidence; transport cancellation and hard deadlines
remain unverified. Clean and conflicting merge examples do not establish parity with
native Git's merge strategy. Gix's full operation coverage is a separate
source audit; this artifact does not demonstrate a gix implementation.

## Published sources

These links pin the source version used by the probe:

- [Repository methods](https://docs.rs/crate/git2/0.21.0/source/src/repo.rs)
  define discovery, linked worktrees, `set_index`, `reference_matching`,
  `merge_commits`, reflog access and reference log creation.
- [Index implementation](https://docs.rs/crate/git2/0.21.0/source/src/index.rs)
  documents `add_all`, ignore handling, tracked ignored files and the
  in-memory index operations.
- [Worktree implementation](https://docs.rs/crate/git2/0.21.0/source/src/worktree.rs)
  defines worktree validation and pruning options.
- [Transaction implementation](https://docs.rs/crate/git2/0.21.0/source/src/transaction.rs)
  documents held loose-ref locks and explicitly states that several
  reference updates are not atomic.
- [Remote implementation](https://docs.rs/crate/git2/0.21.0/source/src/remote.rs)
  defines fetch and push.
- [Remote callbacks](https://docs.rs/crate/git2/0.21.0/source/src/remote_callbacks.rs)
  define `push_update_reference`, including a rejection status for a ref
  the server did not update.
- [Feature declarations](https://docs.rs/crate/git2/0.21.0/source/Cargo.toml)
  make HTTPS and SSH explicit transport choices.

Conductor's comparison points are
[`workspace/plan.rs`](../../../crates/host/src/workspace/plan.rs) and
[`workspace.rs`](../../../crates/daemon/src/workspace.rs). The prototype
keeps its recovery decisions in a data-only `no_std` core and its repository
operations in the executable.
