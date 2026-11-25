# Project operations use Git and repository hosting backends

The owner chose library-backed local Git operations and GitHub issue storage in spec `0v265ymj`, so agents can invoke Conductor's project operations while its backends perform Git and repository hosting actions. The issue model and flow remain independent of GitHub, with provider types converted inside the adapters. This replaces architecture decision D7's subprocess choice for the proposed implementation; git2 is the preferred local candidate, subject to the landing recovery prototype, and Octocrab is the recommended GitHub client.

## Consequences

The local backend must preserve Conductor's worktree ownership, snapshots and landing recovery instead of assuming that libgit2 duplicates every Git command's behavior. Git2 brings a C dependency and does not provide atomic multi-reference commits. Direct GitHub edits remain valid inputs, and issue changes require connectivity; uncertain remote outcomes still require recovery even though offline editing is removed. The current project queue remains the proposed landing process. The [research](../github-backends.md) records the sources and decisions still open.
