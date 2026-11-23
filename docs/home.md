# The home: projects, issues and sessions across the fleet

Status: design, decisions in §9; §8 says what is built. `docs/model.html`
draws §2. It extends the fleet plan (architecture §6) with what the fleet
works *on*.

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

The model names the entities from the domain and how they relate. Nothing
that can be renamed is a key: entities are keyed by ids that never change,
and names are ordinary properties, unique where a person refers to them by
name. Commands and the app take names and look up the id. The rule is for
the owner's own data: an instance's account is named by its provider, not
by the owner, so it is part of the instance's key.

### 2.1 Entities

| Entity | Key | Properties | Parts | Stored in |
|---|---|---|---|---|
| Node | `NodeId` (its public key) | name, relay, ticket | home sync state | fleet config |
| Harness | `HarnessId` (`claude`, `codex`) | name, pinned version | credential forms | the binary (plugin data) |
| Instance | (node, harness, account) | credential, model, CLI path | | a node's store |
| Project | ULID | name (unique), title, root commit (unique), remote, profiles | | home `projects/<id>/project.toml` |
| Issue | ULID | project, title, body, status, priority, labels, branch | notes, checklist | home `projects/<project>/issues/<id>.md` |
| Session | (issue, instance) | status, model, effort | workspace, transcript | a node's log |

Two relations carry data of their own and are keyed by what they join:

| Relation | Key | Properties | Stored in |
|---|---|---|---|
| Pairing | (node, node) | | fleet config |
| Copy | (node, project) | path on the node | a node's store |

### 2.2 Relationships

- Node `1` – `*` Instance; Harness `1` – `*` Instance.
- Node `*` – `*` Node, through Pairing: any paired node may drive any other.
- Node `*` – `*` Project, through Copy: a node has at most one copy of a
  project.
- Project `1` – `*` Issue.
- Issue `1` – `*` Session, and Instance `1` – `*` Session: a session is one
  instance working on one issue, at most one per pair, so two sessions of
  one instance never race on an issue.

A session's node and project follow from its instance and issue, so they
are never stored and can't disagree.

### 2.3 Parts

- **A session's workspace** is what it needs to work on its issue: a git
  worktree of the node's copy of the project, on the session's own branch
  cut from the issue's branch (a branch is checked out in one place at a
  time). It is made when the session starts and removed with the session;
  what the session did reaches the issue by landing its branch on the
  issue's branch. The initial state and each turn's snapshot record a
  commit on the session's branch. An unchanged turn may reuse the same
  commit; per-turn diffs compare consecutive recorded snapshots.
- **A session's transcript** holds its events, checklist, requests and
  sub-agents. A sub-agent is something an agent does inside its session,
  like a tool call, so it can't belong to another issue.
- **The harness's own id for the conversation** (Claude's session id,
  Codex's thread id) is its plugin's bookkeeping, kept in the session's
  workspace, not part of the model.
- **An issue's notes** are an append-only list, its checklist a list of
  items. There are no sub-issues: work is broken down by a checklist or by
  several issues.

### 2.4 Rules the structure doesn't show

- Every session belongs to an issue; a quick, unplanned session gets an
  issue made for it.
- A session's instance runs on a node that has a copy of its issue's
  project.
- Continuing on another instance or node is a new session on the same
  issue, starting from the issue's branch and notes, not from an earlier
  transcript.
- The home never records where a node keeps anything: a node finds its copy
  of a project by root commit and remote. The root commit identifies the
  copy but isn't the key, since a fork or a templated repository shares it.

### 2.5 Lifecycles

```
Issue    as the home's flow declares (docs/agents/flow.toml); by role:
         idle ─► startable ─► active ─► review ─► terminal
            └───────────────────────────────────► dropped
Session  starting ─► idle ⇄ running ─► closed ─► removed (with its workspace)
```

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

**Start a session on an issue.** In the app: pick an issue, then a node,
then one of its instances. The node needs a checkout of the project. Clone
or otherwise provide one before starting a session. The node creates the
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
- **Projects and issues.** A projects view with each project's issues by
  status; an issue shows its checklist, notes and sessions, with *Start a
  session* and *Continue elsewhere*.
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
   (§2.3). Done (architecture §5.3, §5.4). See §4 for the checkout
   and publication prerequisites. Unchanged turns may share a commit;
   finishing lands the session's branch on the issue's branch.
8. Standards and instructions composed into sessions. Done (§5).
9. Agent and sub-agent checklists shown in their sessions, without automatic issue notes (§4).

Each step is usable on its own and keeps the gate green.

## 9. Decisions

- **Nothing that can be renamed is a key.** Ids never change; names do.
- **Issue ids** are global ULIDs, unique across every project and node; the
  app shows an issue by its title.
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
