# Conductor glossary

- **Opening events** record a new session in order. They are
  `SessionCreated`, `OnIssue` for an issue start, `WorkspaceCreated`, and
  `CheckpointCreated`.
- **Initial checkpoint** is the snapshot of a new workspace before its
  agent starts. It has no parent or turn to undo.
- **Issue brief** is the first message a session on an issue tries to send
  to its agent. It gives the agent the issue's context.
