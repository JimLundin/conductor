# Project operations use Git and repository hosting backends

The owner chose library-backed local Git operations and GitHub issue storage in spec `0v265ymj`, and agreed to the [crate structure](../project-backend-structure.md) on 2026-11-25. Project operations use separate version control, issue tracking and repository hosting interfaces, while the issue model and flow remain independent of GitHub. This replaces architecture decision D7's subprocess choice for the planned implementation; gix is the proposed adapter, subject to the library coverage and recovery decision, with git2 as the assessed alternative and Octocrab as the GitHub client.

## Consequences

The local backend must preserve Conductor's worktree ownership, snapshots and landing recovery instead of assuming that libgit2 duplicates every Git command's behavior. Git2 brings a C dependency and does not provide atomic multi-reference commits. Direct GitHub edits remain valid inputs, and issue changes require connectivity; uncertain remote outcomes still require recovery even though offline editing is removed. The current project queue remains the proposed landing process. The [research](../github-backends.md) records the sources and decisions still open.
