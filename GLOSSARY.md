# Conductor glossary

- **Opening events** record a new session in order. They are
  `SessionCreated`, `OnIssue` for an issue start, `WorkspaceCreated`, and
  `CheckpointCreated`.
- **Initial checkpoint** is the snapshot of a new workspace before its
  agent starts. It has no parent or turn to undo.
- **Issue brief** is the first message a session on an issue tries to send
  to its agent. It gives the agent the issue's context.
- **Summary turn** asks a running session's agent to describe its work
  before closing the session and landing its branch. The reply becomes
  a note on the issue.
- **Workspace plan** decides the Git and filesystem steps for a session's
  worktree. Each result determines the next step or completes the plan.
