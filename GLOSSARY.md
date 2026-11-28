# Conductor

Conductor supervises agent conversations on the owner's hosts and keeps
each session's work and progress available to the owner.

## Language

**Session**:
A conversation with an agent on one host for its lifetime. Work continued
on another host starts a new session.

**Workspace**:
The files a session works on, isolated from the owner's original working
copy and from other sessions.

**Turn**:
A stretch of an agent's work that ends by completing, failing or being
interrupted.

**Snapshot**:
A recorded state of a session's workspace before its agent starts or
after a turn ends. Separate turns can have the same workspace state.
_Avoid_: Checkpoint

**Initial snapshot**:
The recorded state of a new workspace before its agent starts. It is the
starting point for the first turn's changes.

**Checklist**:
The latest ordered list of work and its status reported by an agent or
sub-agent. Its entries have no identity across revisions and cannot be
assigned to another session.

**Issue brief**:
The first message a session on an issue tries to send to its agent. It
gives the agent the issue's context.

**Summary turn**:
A turn that asks a running session's agent to describe its work before
closing the session and landing its work. The reply becomes a note on the
issue.

**Pending issue edit**:
A change to an issue that the home has not yet accepted. It remains
pending while the host is offline or the home cannot be updated. It is
known by its edit id, which the app mints once and sends again with each
retry under a new command id, so the home applies it once.
The signed-off backend contract replaces this offline-edit behavior with online
writes and recovery records for attempted operations.

**Issue ID**:
A string unique among the issues of a project's issue tracker, given by that tracker. A user names an issue by its project and its ID, whichever tracker the project uses. Nothing outside the tracker reads meaning into the string.
_Avoid_: issue reference, issue number, slug

**Canvas**:
A structured explanation of an issue's work, written by an agent as typed blocks and kept by the project's issue tracker with the issue.
_Avoid_: report, visual, diagram

**Stray**:
Something a canvas store holds where a canvas belongs that isn't a canvas: a misnamed file, a folder that isn't an issue, or a file that doesn't parse. The store reports each by an opaque place and the reason, and `home check` reports it as an error.

**Client link**:
The contract between a host and the app: the frames each side sends on a
connection, built from the commands, results, refusals and feeds of the
domains it carries.
_Avoid_: protocol, proto, wire

**Fleet protocol**:
The contract between hosts in one fleet, and the signed data they exchange.
_Avoid_: protocol, proto

## Project operations and backends

These terms describe the direction chosen in spec `0v265ymj`. The
backends are not implemented yet.

**Project operation**:
A request to act on a project's work, such as publishing a revision or
landing reviewed changes. Its rules belong to Conductor; its Git and
repository hosting actions belong to the backends it uses.
_Avoid_: raw command, stage

**Issue backend**:
The interface between the issue model and the store that holds its records.
_Avoid_: issue file store

**Version control backend**:
The interface for preparing owned workspaces, preserving and comparing
snapshots, integrating session work and publishing exact revisions.
The Git implementation maps those operations to worktrees, indexes,
commits and refs.
_Avoid_: Git command interface, GitHub backend

**Repository hosting backend**:
The interface for a hosting service's repository checks, statuses and
workflow runs.
_Avoid_: version control backend

**Issue observation**:
An issue's observed state or invalid record, with its identity, findings,
freshness and completeness. An observation does not guarantee that a remote
writer cannot change the issue next.

**Session integration**:
Applying a session's recorded work to its issue branch. It does not by
itself submit that branch to the project's default branch.

**Project landing**:
Submitting a reviewed issue branch through the project's configured
landing process and recording its result. Deployment and issue completion
can have separate outcomes after the code has landed.

**Uncertain issue write**:
An attempted remote issue change whose result has not been established.
Its operation record permits reconciliation, not blind resubmission.
_Avoid_: offline edit
