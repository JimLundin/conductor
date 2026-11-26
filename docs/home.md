# The home: projects, issues and sessions across the fleet

The home organizes the projects and issues that sessions work on.
The [session model](model.html) illustrates their relationships. Sections
8 and 9 record the original build order and design decisions.

The issue model below is being replaced by the home's flow: kinds,
statuses, stages and rules declared as data in the home's
`docs/agents/flow.toml`, which the `home` crate interprets. The home's
`docs/adr/0001` and `0002` record why, and its `GLOSSARY.md` defines the
terms.

## 1. What it adds

Conductor runs agent sessions on nodes, each on one of a node's instances
(an agent harness with one account). The home adds what those sessions work
on:

- **Projects**: the repositories the fleet works in.
- **Issues**: what needs doing in a project. Conductor is the tracker.
- **Standards**: shared instructions and check configs, by language, that
  every session in a matching project follows.

The point is continuity without transcripts. A conversation belongs to one
host, harness and account; the work belongs to the issue, its branch and its
notes. So when a subscription runs low, a new session on the same issue
starts on the API instance, on another node or with another harness, and
picks up from the branch and the notes.

## 2. The model

A session is a conversation with an agent on one host. It has its own
ID, workspace and transcript. Start a session in a Git directory, in a
project with a checkout on the host, or on an issue in that project.

| Work | What it owns |
|---|---|
| Project | A repository and its issues. Each host discovers its own checkouts. |
| Issue | Work that can span sessions. Its branch and notes carry work between hosts and provider accounts. |
| Session | One agent conversation on one host and provider account, with an isolated worktree and a transcript. |

A directory or project start leaves the session's issue unset. An issue
start uses the issue branch and sends its brief to the agent. A host
can have one session assigned to an issue for each provider account.
Removing that session frees the assignment. Continuing a closed session
uses the same conversation. Starting on the same issue elsewhere creates
a new session with its own transcript.

Each workspace has a session branch. Its initial snapshot and each
completed turn's snapshot record commits on that branch. Unchanged
turns can share a commit. Finishing an issue session integrates its
work into the issue branch and records the summary. Sub-agents, requests
and agent checklists belong to the session transcript.

### Host setup

Pairing identifies which hosts the app can reach. On each host, an
instance is a provider and an account with its own credentials, available
models and CLI settings. A project checkout path belongs to the host.
The home identifies the project across hosts by its project ID.

### Lifecycles

The home's flow declares the stages and statuses an issue can move through.
A session starts, becomes idle and runs turns. After it closes, the owner
can continue the conversation or remove the session and its workspace.

## 3. The home repository

The home is the owner's home repository, `JimLundin/home`. Every node keeps
a clone in `<data>/home-repo`.

- **`projects/<id>/project.toml`** describes a project: its `name`,
  `title`, `root` commit, `remote` (absent when local only) and `profiles`.
  The folder is named by the project's id, so renaming a project changes
  only its file. The home never records where a node keeps a copy; a node
  finds its copy by root commit and remote.
- **`projects/<id>/issues/<issue>.md`**: one file per issue.
- **The shared `AGENTS.md`** holds the rules for every session. The
  profiles' checks and configs are a project of their own, `standards`,
  whose `project.toml` lists the profiles it `provides`.
- **One file per thing.** Two nodes editing different issues never touch
  the same file.
- **Append-only where several write.** An issue's notes are a list that
  sessions append to, so concurrent notes merge as a union. Front-matter
  fields have one writer at a time.
- **Sync.** A node pulls before it reads (bounded by a short interval) and
  commits and pushes after it writes, rebasing on conflict. A rebase that
  still conflicts keeps both versions of the note list and takes the newer
  status. A node that can't reach the remote keeps committing locally and
  syncs when it can.
- **No secrets.** Credentials, sign-ins and the fleet's trust stay in the
  signed fleet config.
- **Readable directly.** It is plain files in git, so a project's issues
  read in an editor or on GitHub too.

Issue file writes in `crates/daemon/src/home/issue_file.rs` retain every
ancestor directory handle. On Windows, cap-std operations can resolve a
handle to a path; the handles deny delete sharing, so those ancestors cannot
be replaced while an issue file is open. Paths returned by winx keep their
verbatim drive or UNC prefix unchanged, including `\\?\UNC\` on shares.

### Project file

```toml
name = "conductor"
title = "Conductor"
root = "a1b2c3…"
remote = "https://github.com/JimLundin/conductor.git"
profiles = ["rust"]
```

### Issue file

```markdown
+++
id = "01JD…"
title = "Sign in to a subscription from the app"
status = "active"             # one of the flow's statuses
priority = "high"
labels = ["app", "credentials"]
branch = "issue/01JD…"
created = 2026-11-18T10:12:00Z
+++

What to build and why, as written by whoever opened it.

## Checklist
- [x] the protocol
- [ ] the browser scenario

## Notes
- 2026-11-18 laptop · codex · api: built the protocol; browser scenario
  left; landed on the issue branch.
```

Beyond the id, the title, the kind, the status and the stage, an issue has
the fields its flow declares. Conductor finds the labels, the branch and the
time it was created by the role the flow gives a field (`role = "labels"`,
`"branch"`, `"created"`), never by its name; a flow without a field of a
role has none, and the branch is then `issue/<id>`.

## 4. Flows

**Start a session on an issue.** Open the issue in the app and start a session.
When several host and account combinations qualify, choose one.
The node needs a checkout of the project. The node creates the
session's workspace from the issue branch and gives the agent the issue's
text, checklist, notes and instructions for finishing.

**Continue elsewhere.** Start a new session on the same issue to use its
branch and notes. To carry landed commits to another node, first publish the
issue branch to the project remote and make sure the target node has a
checkout that can fetch it.

**Continue this session.** Select **Continue** after its agent stops or
the host restarts. The session resumes its provider conversation on the
same node, with its workspace and recorded turn snapshots. Opening a
turn's diff does not change its files or conversation.

**Finish.** Select **Finish** in the session. The node asks the agent for a
summary, stops it and tries to land the session branch on the issue branch.
It adds the summary to the issue notes and moves the issue to `review` when
landing adds commits. Check the transcript's **Finished** row for the landing
and push result. If it says the branch was kept on this host, inspect
`issue/<id>` in the source checkout and check the remote configuration or
reported push error before trying to continue on another node. Removing the
session removes its workspace.

**Agents updating issues.** The summary turn at the end becomes a note on
the issue. Explicit issue notes remain available. Agent and sub-agent
checklists show their latest ordered progress inside the session; completing
an item does not write a note or change an issue.

## 5. Standards and instructions

A profile's checks and configs are in `<profile>/` of the project that
`provides` it, the standards project, and the shared `AGENTS.md` holds
the rules for every session. A project names its profiles in
`project.toml`. When a session starts, the node composes its instructions
from the shared rules, the project's profiles and the project's own
`AGENTS.md`. The harness plugin says how its CLI takes them, for example an
appended system prompt or a generated instructions file in the workspace.

Built (`home::instructions`). The shared rules are the home's root
`AGENTS.md`, and the composed text says where the node's clone is, so the
rules' paths into the home resolve. Each profile is named with the path of
`<profile>/` and its `check.sh` in the node's first checkout of the
project that provides it, or as provided by no checkout here. The session's project is the
one with a checkout at or around the directory it was created in, through
any symlink; a session elsewhere, or one whose agent starts before the
node first reads its home, gets the shared rules only. The project's
`AGENTS.md` is read where the agent runs, its workspace on the session's
branch, and left out when that holds the file the CLI reads by itself,
which its plugin names (`CLAUDE.md` for Claude, `AGENTS.md` for Codex): a
`CLAUDE.md` is taken to bring in the project's rules. The session's actor
composes them each time its agent starts, from the files as they are
then, so an edit to the standards reaches the next start;
Claude takes them as an appended system prompt, Codex as the thread's
developer instructions (architecture §3).

## 6. Sub-agents

Built (architecture §3). They are part of a session's transcript: the tool
call that started each, a description, its own transcript and status.

- Claude: `system/task_started` of a `local_agent` starts one, its frames
  carry `parent_tool_use_id` and `agent_id`, and `task_notification` ends
  it (verified in a recording and the pinned binary, Claude `PROTOCOL.md`).
- Codex: the pinned 0.166's app-server schema has collab agents, each a
  thread of its own, but no run has recorded one, so they are not mapped
  yet (Codex `PROTOCOL.md`).

## 7. The app

- **Several nodes.** The app keeps every paired node and shows each one's
  sessions, instances and availability. Done.
- **Projects and issues.** Expand a project in the sidebar to open an issue or
  start a session in the project. An issue shows its brief and offers the
  accounts that can start a session on it.
- **Renaming.** Projects and nodes are renamed, a project by an edit of
  its `project.toml`; a rename changes only the name, never what refers to
  it. An instance isn't renamed: it is its account.

## 8. Build order

1. Sub-agents in the session model. Done.
2. The app with several nodes. Done.
3. The home repository: clone, sync, and projects with their copies. Done
   (architecture §5.4).
4. Ids instead of names: projects in `projects/<id>/` with a unique name
   and a title, and instances keyed by their provider's account. Done
   (architecture §5.2, §5.4).
5. A session's workspace removed with the session, keeping its branch.
   Done (architecture §5.3).
6. Issues: create, edit, status, checklist and notes in the app. Done
   (architecture §5.4).
7. Sessions on issues: start, finish, continue elsewhere, with the
   session's branch cut from the issue's and each turn recording a commit
   (§2). Done (architecture §5.3, §5.4). See §4 for the checkout
   and publication prerequisites. Unchanged turns may share a commit;
   finishing lands the session's branch on the issue's branch.
8. Standards and instructions composed into sessions. Done (§5).
9. Agent and sub-agent checklists shown in their sessions, without automatic issue notes (§4).

Each step is usable on its own and keeps the gate green.

## 9. Decisions

- **Nothing that can be renamed is a key.** Ids never change; names do.
- **Issue ids** are 8 lowercase base 32 digits, global across every
  project and node: 6 count the seconds since 2026-01-01T00:00:00Z, so ids
  sort by creation, and 2 are random, drawn again when the home already
  has the id. A project id is 6 random digits. Any other form is a
  structural problem. The app shows an issue by its title.
- **The home is `JimLundin/home`.**
- **Conductor is the tracker.** It is the only place issues are written. A
  one-way mirror to GitHub issues may come later, but nothing is read back
  from it.
- **Any paired node works on any node.** Nodes form one small network:
  every paired node may create issues and start sessions on any node and
  instance. Trust is the fleet's certificates (architecture §6.1); there
  are no per-node roles.
- **One session per instance per issue**, and a session's workspace lives
  and goes with it; there is no idle cleanup.
