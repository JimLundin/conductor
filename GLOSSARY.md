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
- **Pending issue edit** is a home mutation kept on the host until its
  upstream has accepted it. Replay can apply it to an updated issue file
  or let a newer change to the same field stand.
- **Line-JSON process runner** owns a CLI child and its pipes. It delivers
  stdout lines and I/O outcomes to the protocol driver and runs its effects.
- **Protocol driver** translates CLI frames and handle commands into
  ordered effects and tracks replies that are waiting on the CLI.
- **Host fold** is the client’s read model for one host. It takes link
  updates and keeps the host’s latest welcome, sign-ins, home, instance
  roster, and folded sessions.
- **Shared configuration acceptance** is the root host’s durable agreement
  to a change in the fleet’s intended configuration. It does not promise
  that any host can use the change yet.
- **Shared configuration application** is one host’s adoption of an
  accepted shared configuration change. Its progress and outcome are
  separate from acceptance.
- **Instance availability** is whether a host can currently run work with
  an instance. A host can adopt shared configuration while its affected
  instances remain unavailable.
- **Instance retirement** withdraws an instance from new work and stops
  its agents within a bounded grace period. Work still running when that
  period expires is stopped forcibly.
