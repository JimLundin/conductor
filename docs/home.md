# The home: projects, issues and work across the fleet

Status: design, decisions in §9. Step 1 of §8 is built; the rest is not yet.
It extends the fleet plan (architecture §6) with what the fleet works *on*.

## 1. What it adds

Today Conductor runs sessions on hosts, each on one of a host's instances
(an account with a provider). The home adds the work those sessions do:

- **Projects**: the repositories the fleet works in.
- **Issues**: what needs doing in a project, the tracker.
- **Work**: one attempt at an issue, by one agent, on one host and
  instance. An issue can take several attempts, on different instances,
  providers or hosts, each continuing from the last.
- **Standards**: shared instructions and check configs, by language, that
  every session in a matching project follows.
- **Sub-agents**: the agents a session starts, shown under it.

The point is continuity without transcripts. A conversation belongs to one
provider and one account. Work belongs to the issue: its branch, its
commits and the notes each attempt leaves. So when a subscription runs low,
the next attempt starts on the API instance, on another host or with
another provider, and picks up from the branch and the notes.

## 2. The model

```
Fleet                      architecture §6: trust, hosts, credentials (signed, secret)
└── Home                   one git repository, cloned on every host (no secrets)
    ├── standards/         profiles: instructions, CI and check configs per language
    ├── instructions/      rules for every session in every project
    └── projects/<project>/
        ├── project.toml   remote, root commit, profiles, default branch
        ├── issues/<issue>.md
        └── work/<issue>/<work>.toml

Host (each machine)        host-local, in its own store
├── Instances              built: provider, label, credential or sign-in
├── Checkouts              project → path on this host
└── Sessions               built: one agent conversation, now with sub-agents
```

| Entity | Id | Lives in | Written by |
|---|---|---|---|
| Project | slug (`conductor`) | home `project.toml` | the app |
| Issue | ULID, globally unique; shown by its title and a short prefix of the id | home `issues/` | the app, and agents through Conductor |
| Work | ULID | home `work/` | the host running it |
| Standard profile | slug (`rust`) | home `standards/` | the owner, by commit |
| Checkout | project id on a host | host store | the host |
| Instance, session | as today | host store and log | the host |
| Sub-agent | the provider's id, under its session | host log | the host |

Ids are ULIDs where several hosts create them at once, so two hosts never
pick the same id and merging the home never collides on a name.

## 3. The home repository

The home is the owner's home repository, `JimLundin/home`, which already
holds `standards/`, the shared `AGENTS.md` and the list of projects
(`repos.sh` and the table in `AGENTS.md`). Every host keeps a clone in
`<data>/home`. Adopting it takes a few changes there:

- `projects/<project>/` holds the tracker. Today the project repositories
  themselves are checked out beside `standards/` and ignored by the home's
  `.gitignore`; the new `projects/` folder is tracked and holds only
  Conductor's files, never a checkout.
- `project.toml` becomes the list of projects, and the `AGENTS.md` table and
  `repos.sh` read it instead of keeping their own.
- `instructions/` holds what the shared `AGENTS.md` says today, split into
  the rules for every session and those for one profile.

- **One file per thing.** An issue is one Markdown file with TOML front
  matter; a work record is one TOML file. Two hosts editing different
  issues never touch the same file.
- **Append-only where several write.** An issue's progress notes are a list
  that attempts append to, never rewrite, so concurrent notes merge as a
  union. Front-matter fields have one writer at a time: the app sets
  status and assignment, and the running work updates only its own file.
- **Sync.** A host pulls before it reads (bounded by a short interval) and
  commits and pushes after it writes, rebasing on conflict. A rebase that
  still conflicts keeps both versions of the note list and takes the newer
  status, recording the conflict in the issue. A host that can't reach the
  remote keeps committing locally and syncs when it can; later this can
  fall back to a bundle over `conductor-fleet`, as session moves do
  (architecture §6.8).
- **No secrets.** Credentials, sign-ins and the fleet's trust stay in the
  signed fleet config. The home is safe to read on any device.
- **People and agents can read it directly.** It is plain files in git, so
  a project's issues are readable in an editor or on GitHub too.

### Issue file

```markdown
+++
id = "01JD…"
title = "Sign in to a subscription from the app"
status = "in-progress"        # open, ready, in-progress, review, done, dropped
priority = "high"
labels = ["app", "credentials"]
parent = "01JC…"              # optional: a sub-issue
created = 2026-11-18T10:12:00Z
+++

What to build and why, as written by whoever opened it.

## Notes
- 2026-11-18 desktop/codex (work 01JD…): built the protocol; browser
  scenario left; branch conductor/01JD….
```

### Work file

```toml
id = "01JD…"
issue = "01JD…"
host = "desktop"
instance = "codex"
session = "…"                 # the host session that runs it
branch = "conductor/01JD…"
continues = "01JC…"           # the attempt it picked up from, if any
status = "running"            # running, stopped, finished, abandoned
started = 2026-11-18T10:20:00Z
ended = 2026-11-18T11:05:00Z
outcome = "pushed"            # pushed, merged, nothing, failed
summary = "…"                 # the agent's own account, written at the end
```

## 4. Flows

**Assign.** In the app: pick an issue, then a host, then one of its
instances. The host checks it has a checkout of the project (or clones it),
writes the work record, creates the worktree on a fresh branch (or on the
continued attempt's branch), and starts a session whose first message is
built from the issue: its text, its notes, and what to do when done.

**Continue on another instance.** The same flow with *Continue from* an
earlier attempt: the new work's branch starts from the old one's head, and
its first message adds that attempt's summary and notes. Across hosts the
branch travels through the remote, or a bundle when there's none.

**Finish.** When the agent ends its work (or the person stops it), the host
asks it for a summary, appends a note to the issue, sets the work's
outcome, and moves the issue to `review` when there's a branch to look at.

**Agents updating issues.** Agents don't edit the home checkout. They report
through Conductor, as data the session already carries: the summary at the
end and progress notes during. A later step can give them a small tool for
it (§7).

## 5. Standards and instructions

`standards/<profile>/` holds a profile's check configs (as `standards/`
does today), and `instructions/` holds rules for every session. A project
names its profiles in `project.toml`. When a session starts in a project,
the host composes its instructions:

1. `instructions/` (every session),
2. each of the project's profiles,
3. the project's own `AGENTS.md`.

The plugin says how its CLI takes them (data in its profile): for example
an appended system prompt or a generated instructions file in the
worktree. Checks run the profile's `check.sh`, so a Rust project on any
host gets the same gate and CI definitions.

## 6. Sub-agents

Built (architecture §3). A session's sub-agents are child sessions in the
model: an id under the parent, the tool call that started them, a
description, their own transcript and status. Their events are the
parent's, in its log; the registry has no session for them. The provider's
core maps them:

- Claude: `system/task_started` of a `local_agent` starts one, its frames
  carry `parent_tool_use_id` and `agent_id`, and `task_notification` ends
  it (verified in a recording and the pinned binary, Claude `PROTOCOL.md`).
- Codex: the pinned 0.166's app-server schema has collab agents, each a
  thread of its own, but no run has recorded one, so they are not mapped
  yet (Codex `PROTOCOL.md`). The model takes them as they are.

The app shows each under the tool call that started it, collapsed to its
description and status, and opens to its transcript. They are not work
records: their output belongs to the parent's attempt.

## 7. The app

- **Several hosts.** The app keeps every paired host (it holds one ticket
  today) and shows each one's instances and availability.
- **Projects and issues.** A projects view, each with its issues by status;
  an issue shows its notes and attempts, with *Assign* and *Continue*.
- **Sessions** stay as they are, reached from their work or from the host,
  with sub-agents nested.

## 8. Build order

1. Sub-agents in the session model (host only; no home needed). Done.
2. The app with several hosts.
3. The home repository: clone, sync, and projects with checkouts.
4. Issues: create, edit, status, notes, in the app.
5. Work: assign an issue to a host and instance, finish, continue.
6. Standards and instructions composed into sessions.
7. Agents reporting progress notes during work.

Each step is usable on its own and keeps the gate green.

## 9. Decisions

- **Issue ids** are global ULIDs, unique across every project and host. There
  are no per-project numbers to collide; the app shows an issue by its title
  and a short prefix of its id.
- **The home is `JimLundin/home`**, adjusted as §3 says.
- **Conductor is the tracker.** It is the only place issues are written. A
  one-way mirror to GitHub issues may come later, the way the plugins mirror
  each CLI's own interface, but nothing is read back from it.
- **Any paired device assigns work on any host.** Clients and hosts form one
  small network: every paired client may create issues and assign work to
  any host and instance, and a host may assign work to another host (for
  example an agent handing a sub-issue on). Trust is the fleet's device and
  host certificates (architecture §6.1); there are no per-device roles.
