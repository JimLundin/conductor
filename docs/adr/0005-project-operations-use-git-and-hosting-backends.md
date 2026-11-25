# Project operations use Git and repository hosting backends

The owner chose library-backed local Git operations and GitHub issue storage in spec `0v265ymj`, and agreed to the [crate structure](../project-backend-structure.md) on 2026-11-25. Project operations use separate version control, issue tracking and repository hosting interfaces, while the issue model and flow remain independent of GitHub. This replaces architecture decision D7's subprocess choice for the planned implementation.

Use git2 0.21.0 for the first version control adapter and Octocrab 0.54.9 for GitHub. Decision `0v26ayew` settles the provisional gix choice after the source audit found no push execution API in gix 0.91.0. The [local prototype](../experiments/vcs-probe/README.md) verifies scratch snapshots, worktrees, merges, local fetch and push, and recovery between completed landing writes.

The owner clarified during build that the version control interface models project work. Its callers request owned workspaces, snapshots, session integration and publication of an exact revision. Git indexes, object construction, worktrees and ref updates stay inside the implementation. A typed Git command interface is not the domain boundary.

## Consequences

Git2 brings a C dependency and does not provide atomic multi-reference commits. Local landing persists an attempt before changing its branch, then repairs or returns its receipt on retry. Changed or missing history can require reconciliation. The production adapter must verify transport authentication, server rejection, required merge behavior and coordination with external writers. The prototype does not establish power-loss durability or universal exactly-once execution.

Direct GitHub edits remain valid inputs, and issue changes require connectivity; uncertain remote outcomes still require recovery even though offline editing is removed. The current project queue remains the proposed landing process. The [research](../github-backends.md) records the sources and decisions still open.
