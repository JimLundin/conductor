# Snapshot installation and recovery policy

Decision `0v2cse85`, spec `0v265ymj`. This extends the existing git2 `0.21.0`
probe. It changes no production API or dependency. The runnable evidence is
in [installation.rs](src/installation.rs), [features.rs](src/features.rs)
and the existing [integration recovery](src/recovery.rs). The decisions
are data-only in the [no_std core](core/src/installation.rs).

## Decision

Use retained candidate refs, immutable operation intents and separately
recorded index/ref progress. Restrict installation to an owned session
checkout while its agent and other writers are stopped. Refuse unsupported
repository modes before capture, workspace creation or integration.
Detected external changes require reconciliation. Ref and index locks
do not establish exclusive ownership of a checkout.

Installation means recording the captured files as the session branch tip
and its index. It never checks out a tree or restores files. The source
checkout remains unchanged, including partially staged source work. The
session index can be replaced only when its tree equals the previous
session tip or the candidate. Partial staging in the session is a refusal.

This preserves the current operations in
[workspace/plan.rs](../../../crates/host/src/workspace/plan.rs). Its
`snapshot` first retains the candidate, then calls `sync_index`, then
conditionally updates the session branch. It has no durable installation
intent between those effects. The new probe supplies evidence for that
missing boundary.

## Evidence and its limits

The executable covers these cases in disposable repositories without a
Git executable. The fixture uses a separate linked session checkout and a
source checkout with staged and unstaged changes.

| Case | Required observation |
|---|---|
| Installation boundaries | Reopen after intent, index-begun marker, index replacement, branch-begun marker, branch publication and receipt. Resume reaches the candidate without rewriting files. |
| Original checkout | Its HEAD bytes, real index bytes and dirty file bytes remain unchanged after each boundary. Session symbolic HEAD bytes also remain unchanged. Its branch target advances intentionally. |
| Session staging | Clean and fully staged indexes install. A partially staged index refuses preparation before candidate retention or index/ref installation. Its original bytes remain intact. |
| External changes | Changed file bytes, a detached HEAD, changed index bytes, a branch rewind after publication and a replacement checkout directory refuse continuation. An interrupted rewrite never rolls the installed index back. |
| Completed retry | A receipt returns completion after a later file edit and HEAD change, without repairing those later edits. |
| Retention | A ref walk from the prepared namespace reaches the snapshot and integration candidates. Removing the required candidate ref refuses unfinished work. |
| Locks | A pre-existing index lock or ref lock refuses writes and keeps its bytes. Holding the session branch lock still allows another handle to detach HEAD. Holding Git locks still allows the directory to be moved and replaced. |
| Required attributes | `text=auto eol=lf` normalizes CRLF text when captured. Explicit binary and auto-detected binary retain bytes. Linked checkout writes LF text and preserves binary bytes. Capture does not rewrite the source CRLF file. |
| File modes | Capture and linked checkout preserve executable mode and a file symlink on Linux. |
| Binary merge | Disjoint ASCII edits in a file marked `binary` produce a conflict and leave HEAD unchanged. The attribute prevents an otherwise possible textual merge. |
| Custom filter gap | A configured required external clean/smudge filter is not run by this git2 capture; capture succeeds with raw bytes. Production must detect and refuse that configuration. |
| Blocking cancellation | Dropping a dispatched thread's join handle leaves its reference update running. A completion channel then confirms the effect. No claim of stopping an in-progress libgit2 call follows from cancelling its caller. |

These are controlled pauses after completed effects. They do not kill a
process during an index or ref write, simulate disk errors, prune objects,
or simulate power loss. A successful reopen establishes observable state
on this Linux filesystem. It does not establish storage durability.

## Required current configuration

The current [Git shell](../../../crates/daemon/src/workspace.rs) removes
inherited `GIT_*` variables, disables hooks and fsmonitor, sets
`GIT_TERMINAL_PROMPT=0`, and supplies Conductor's commit identity. It uses
Git's normal config and attributes for capture and `merge-tree
--write-tree` for integration. Replacing it must not silently inherit
different environment discovery, credential-helper or executable-filter
behavior.

At source baseline `1e379bf5`, Conductor's `.gitattributes` specifies
`* text=auto eol=lf`, with PNG, JPG and ICO marked `binary`. The home has
`* text=auto eol=lf`. A restricted read of effective non-secret config in
these two checkouts found `core.filemode=true`; it found no explicit
autocrlf/eol/safecrlf, sparse, index, extension or selected merge overrides.
The effective config does contain installed `filter.lfs` clean, smudge,
process and required keys. The tracked attributes shown above do not
select LFS. An installed filter definition is distinct from an effective
filter attribute on a captured or checked-out path. Preflight must refuse
an active unsupported filter, and must not reject every repository merely
because Git LFS is installed on the host.
This is an inventory of these checkouts, not a claim about every project
the host can discover. Configuration and attributes must be observed
again for each project and each operation.

| Mode | First replacement policy |
|---|---|
| SHA-1, loose/packed refs, ordinary complete index, complete local object database, initialized branch | Supported scope. Local filesystem and linked worktree behavior need the checks above. Ref recovery also requires readable expected reflog evidence. |
| Text, binary, `text=auto`, `eol=lf`, executable files, file symlinks, ordinary ignore patterns | Required. The new attributes/mode cases and previous tracked/untracked ignore cases cover these on Linux. Verify relevant attributes from the worktree, index/commit and info/global sources before effects. |
| Ordinary clean/conflicting two-parent merges | Existing disjoint and same-path conflict evidence remains valid. Binary attributes prevent automatic textual merging. This is not parity with native Git's current merge strategy for every history. |
| Active external filters, LFS attributes, custom merge drivers, working-tree encodings, unsupported attributes or merge configuration | Refuse before effects. Never silently treat a required filter's output as raw content. Configured filters can become active through global attributes; inspect both config and effective attributes. |
| Submodules/gitlinks, nested repositories, sparse checkout/index, split index, assume-unchanged/skip-worktree entries, unresolved index conflicts | Refuse until focused preservation/ownership tests establish support. Do not expand or normalize a special index as a side effect of preflight. |
| Alternates, shared object stores, shallow/partial/promisor clones, replacement refs/grafts, SHA-256 or unsupported repository/ref format | Refuse until object availability, retention and history semantics are verified. A candidate ref cannot keep objects alive in an alternate repository that prunes them independently. |
| Non-Linux filesystems, case-folding/path aliases, symlinked ownership directories, external checkout writers | No safety claim from this probe. Require separately verified platform behavior; refuse active competing writers and reconcile detected changes. |
| HTTPS/SSH, credential refresh, push negotiation and server rejection | Separate transport decision. This executable has those transport features disabled. |

The production preflight reader and refusal mapping remain implementation
work. This artifact deliberately exercises the custom-filter gap directly;
it is not an implementation of that reader. Configuration snapshots,
effective attribute scanning, repository features and checkout ownership
must be bound to the operation's preconditions. A changed or unreadable
precondition is a refusal, not a default to ordinary mode.

## Intent, reference and file policy

Use one operation ID within the owner host's shared CLI/daemon store. Bind
it to the canonical common repository and session identity before effects.
Hold the host's repository-operation lock for the operation. That lock
serializes Conductor callers only. Stop the agent before capture and keep
the checkout quiescent through installation. No scanner or Git ref lock
can exclude arbitrary external filesystem writers.

1. Preflight the repository, source/session ownership and staging. Under
   coordinated maintenance exclusion, construct the candidate and retain
   it at an operation-owned `refs/conductor/prepared/<operation>` ref.
   Never overwrite another operation's ref. A crash before pinning leaves
   unreferenced objects but no authorized installation. A crash after
   pinning and before intent leaves an orphan pin to reconcile later.
2. Persist an immutable versioned intent before index/ref installation.
   Record expected branch/OID and HEAD identity, candidate OID and pin,
   repository/worktree identity, original and proposed index bytes or
   retained verified blobs, relevant configuration and file observations,
   baseline reflog evidence and the eventual result. The fixture records
   only its three changed/ignored files and directory inode; production
   needs complete path/type/mode observations and bounded storage.
3. Create an index-begun record with no replacement and synchronize it.
   Reopen after interruption and compare actual index bytes with both
   recorded forms. An unrelated form refuses. Hold `index.lock` from the
   precondition read through replacement. Write the candidate index to
   that owned lock file, synchronize it, rename it over `index`, then
   synchronize the containing directory. The prepared bytes come from
   git2's index writer in a private directory. No workspace files change.
4. Record branch-begun before publication. Lock branch and HEAD, reobserve
   identity and index, then update only the session branch from the
   expected OID. A candidate index with the old branch resumes publication
   only with unchanged baseline branch history. A published candidate
   with that exact index repairs the receipt. Any incompatible index,
   branch, HEAD or file observation requires reconciliation.
5. Install a receipt ref separately. Its matching intent retains the
   original result. Completed retries return that result even after later
   user edits. They never restore the old index or branch. The existing
   integration protocol uses the same candidate pin requirement and keeps
   its count and receipt semantics.

The fixture syncs intent and marker files and their containing directory,
including the existing parent after journal-directory creation. It uses
`persist_noclobber` for immutable records. Errors keep confirmed progress;
no rollback overwrites later work. Production must validate intent
version, ownership, bounds and checksums before interpreting it. Corrupt
or missing records require reconciliation.

Keep candidate refs while an attempt is incomplete or uncertain. After
completion, retain snapshot refs and receipts for the corresponding
session/result lifetime. Remove prepared pins only after verifying the
durable replacement retention and completed intent. Intent cleanup, ref
cleanup and garbage collection require the same maintenance coordination.
A JSON OID alone is not a garbage-collection root. Normal Git reachability
protects pinned candidates; neither an arbitrary external `prune` nor
external deletion of the pin is controlled by Conductor. Actual GC,
cleanup races and object corruption remain untested here.

## Locks, cancellation and interruption

Do not steal `index.lock`, `HEAD.lock` or ref locks because of their age.
Git lockfiles do not carry a reliable process-owner identity. After a
process crash, report the blocked operation and preserve the lock until
the owner has established that no writer remains. Conductor's host lock
should use a kernel-released interprocess lock with an operation record;
its release does not authorize deletion of unrelated Git lockfiles. This
prototype tests refusal, not production stale-lock recovery.

Observe cancellation before dispatch. Once a blocking effect has started,
the worker owns its repository handle, buffers, lock guards and operation
record until it returns. Caller timeout/cancellation reports in-flight
progress and an operation handle; it cannot report no effects. Join or
observe the worker before cleanup or another attempt. Record confirmed
effects even if the caller has cancelled. A lost worker result remains
unknown until state is reconciled. Callback cancellation is cooperative
where the specific API supplies it and does not imply rollback.

Libgit2's `Transaction` explicitly does not make several ref writes
atomic. Ref locks also do not lock another worktree's HEAD or any directory.
The probe's HEAD and inode observations catch the staged counterexamples;
an external writer can still act between an observation and a write.
Support therefore requires an exclusively controlled session checkout.
If arbitrary simultaneous external checkouts are a requirement, the
contract needs a stronger coordination mechanism and further evidence
before production replacement. The existing refusal for integration into
a checked-out issue branch remains necessary but is also an observation,
not a distributed checkout lock.

No power-loss guarantee is established. Libgit2 has separate object/ref
fsync controls, and its index writer uses a lockfile without the fsync
option in the inspected source. Synchronizing the application journal
cannot make all prior library object/ref writes durable. Production must
either verify object/ref/index ordering on each supported filesystem or
report corrupted/missing recovery evidence as uncertain and preserve the
remaining records. Reopening handles is insufficient evidence to claim
otherwise.

## Primary sources

- [git2 0.21.0 Index](https://docs.rs/crate/git2/0.21.0/source/src/index.rs)
  documents index writes with an atomic file lock and writing an index
  tree without checking out files.
- [git2 0.21.0 Transaction](https://docs.rs/crate/git2/0.21.0/source/src/transaction.rs)
  documents loose-ref lock lifetime, separately committed effects and
  the absence of multi-reference atomicity.
- [libgit2 1.9.7 index writer](https://github.com/libgit2/libgit2/blob/v1.9.7/src/libgit2/index.c)
  contains `git_indexwriter_init` and `git_indexwriter_commit`.
- [libgit2 1.9.7 ref backend](https://github.com/libgit2/libgit2/blob/v1.9.7/src/libgit2/refdb_fs.c)
  gates ref fsync through its configured backend option.
- [libgit2 1.9.7 filters](https://github.com/libgit2/libgit2/blob/v1.9.7/src/libgit2/filter.c)
  registers CRLF and ident as built-in filters. Custom filter registration
  belongs to the embedding application.
- [libgit2 1.9.7 merge drivers](https://github.com/libgit2/libgit2/blob/v1.9.7/src/libgit2/merge_driver.c)
  selects the built-in binary driver through the `merge` attribute.
- [Git garbage collection](https://git-scm.com/docs/git-gc#_notes)
  describes retention through refs and the race with concurrent object
  creation. The probe verifies reachability, not a garbage collector.
- [git2 checkout callbacks](https://docs.rs/crate/git2/0.21.0/source/src/build.rs)
  document cancellation through a notification callback returning false.
- [Rust JoinHandle](https://doc.rust-lang.org/std/thread/struct.JoinHandle.html)
  documents that dropping the handle detaches its thread.
- [Linux fsync](https://man7.org/linux/man-pages/man2/fsync.2.html)
  distinguishes synchronizing a file from its directory entry.

The production gate still needs the repository-feature reader, complete
file/configuration observations, host lock ownership, malformed-intent
handling, actual process-death and I/O fault injection, retention cleanup,
and supported-platform durability evidence. These requirements belong to
the adapter and runtime implementation tasks; the decision does not mark
acceptance 9 or 11 complete for production.
