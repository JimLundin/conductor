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
pending while the host is offline or the home cannot be updated.
