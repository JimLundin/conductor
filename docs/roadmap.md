# Roadmap

What is built and what comes next. Update it in the commit that changes it.

## Built

- **Inactive branch consolidation.** The architecture reviews are reconciled
  against the retained product in [the consolidation assessment](reviews/consolidation.md).
  Their completed cuts remain in place. Remaining cleanup borrows question
  and credential drafts, uses the log's command kind for routing, encodes
  Codex calls without duplicate parameter records.
- **`home repos` and `home skills`.** The `home` command takes over the
  home's `repos.sh`: `home repos` shows each project's checkouts under the
  home's folder, the home among them, by root commit and remote, with each
  one's branch and changes; `sync` clones a project with a remote and no
  checkout into `<home>/<name>`; `scan` registers each repository matching
  no project under a fresh `ProjectId` and rewrites the `AGENTS.md` table;
  `home skills` installs the home's `skills/` in `~/.claude/skills` and
  `~/.agents/skills` through `home_files::install`, which the host also
  runs each time its clone of the home moves. `home::repos` decides; the
  shell runs only Git.

- **Short issue and project ids.** An issue id is 8 lowercase base 32
  digits, the seconds since 2026-01-01T00:00:00Z then 2 random, drawn
  again when the home has it, so issues list in creation order; a project
  id is 6 random digits. `home::IssueId` and `home::ProjectId` mint,
  parse and print them, the `home` command and the daemon mint the same way,
  and a 26-character id is a structural problem (ALPN `conductor/45`).

- **One provider identity and current storage only.** Pin selection and
  installation use `Provider`, and sessions derive their provider from their
  instance throughout the event log, recovery and client view. Requests have one durable source, the event
  log; the write-only request table is gone. Browser pairing reads only
  current per-host records, keeping concurrent-tab writes and fleet revocation
  without single-ticket or shared-map migrations. Older session databases
  must be set aside; browsers with only obsolete pairing records must pair
  again. Historical review reports remain in Git history.

- **CLI pins shipped with Conductor.** Every host uses the CLI versions verified
  with its Conductor release. Removed fleet revision publishing, separate
  fleet installs and revision-driven instance reconfiguration. Host and
  instance model defaults and session selection remain; Codex analytics
  stays off. Fleet membership, shared credentials and credential refresh
  remain, with fleet protocol `conductor-fleet/2`. Previous sealed fleet
  state and backups must be set aside before re-enrollment
  (`docs/fleet.md`).

- **Sessions stay on their starting host.** Removed cross-host session
  migration, branch bundles, transcript archives, ownership recovery and
  move controls. Fleet membership, pairing, credentials and independent
  sessions on multiple hosts remain. Local Continue, restart and per-turn
  diffs remain. Continue work elsewhere with a new session using
  the issue and its remote branch (architecture §6.8).

- **Session-local checklists.** Main and child checklists are ordered
  snapshots of text and status, retained through replay and Continue.
  Task handoff, persistent item identities and automatic checklist-derived
  issue notes are removed. Explicit notes and the summary turn remain.

- **The shared Rust standard.** The subset is retired; `tools/check.sh`
  runs the standard's checks. The core crates are `link`,
  `agents`, `claude`, `codex`, `host` and `home`; `net`, `daemon`,
  `conductord`, `runtime`, `claude-cli`, `codex-cli`, `install`, `home-cli`,
  `home-files` and `web-iroh` declare themselves the shell.
  `host` holds
  workspace decisions and checklists. The shell still makes about a
  quarter of the decisions (Next, item 1).

- **Providers as plugins** (§5.2, §6.3). The daemon library takes a map of
  `runtime::Plugin`s and names no provider: each is its `agents::Profile`
  as data (CLI pin, probes, sign-in, home config, label, note and credential
  forms) plus an adapter for each account. The adapter applies credentials,
  discovers models and starts session agents. Credentials are generic form values the app draws
  from `Welcome.catalog`, checked by their form and read by each
  provider's core. The binary moved to `crates/conductord`, the one place that wires the
  providers, with `--cli <provider>=<path>`, `--model <provider>=<model>`
  and `login` (now per instance, below). The pins in `agents::pin` and
  `Provider` itself are the last places a new provider is listed (Next,
  item 3).

- **Instances** (§5.2, §6.2, §6.3). A host runs instances, each one
  account of a provider, such as Claude through a gateway beside Claude on
  a subscription, keyed by the provider and the account, which the
  provider names (home step 4). A credential's form names its account (a
  gateway's URL); a sign-in names it once the CLI reports who it signed in
  as, and the provider's own instance, where the sign-in ran, becomes that
  account's. Each instance has its own credential, its own private home
  `<data>/home/<dir>`, its own work dir and its own agents, so a Codex
  instance gets its own app-server. Sessions start on an instance and
  record it, so Continue stays on it. The app's credentials screen
  lists the instances by account, takes a credential per provider and
  removes an instance (refused while a session runs on one), and the
  new-session picker lists instances. `--cli` and `--model` are each
  provider's defaults, and `conductord login <provider> [--account
  <account>]` signs one in on the host; the app signs one in too. CLI
  pins come from the Conductor release; host model defaults and session
  model choices remain. Per-instance model and CLI settings from the app
  are Next, item 2.
  *Try:* save a gateway in *Credentials* and see its URL listed as an
  instance beside the provider's own.

- **One agent runtime** (§5.2). The daemon asks one interface for agents:
  `agents::runtime`'s commands and events over a `runtime::Runtime`
  channel pair, with Claude and Codex as two plugins behind it. Each
  provider's core translates the protocol into its wire as a state
  machine, Claude's resume fallbacks included, and `host::session` and the
  daemon's session know no provider. Logging moved out of the cores into
  the shell, and the shell code bent to the old complexity measure is
  plain `async fn` and `select!` again. Each shell package exposes its
  plugin constructor. Recorded replays cover Codex session routing and
  approval replies through the shared runtime.

- **Managed CLIs** (#1) (§6.2). `conductord` installs the pinned Claude
  Code and Codex into its data dir, checks their hashes and runs only those,
  with private `CLAUDE_CONFIG_DIR` and `CODEX_HOME` and updates off. Gateway
  credentials run them. Subscription
  sign-in from the app is built (below); `conductord login <instance>`
  remains as the host-terminal fallback.

- **Responsive provider refresh** (#23). Credential saves, model discovery
  and Codex reconfiguration no longer hold the registry or the global
  command transaction while a CLI responds. Refreshes are serialized per
  instance, coalesce superseded configurations and discard stale results.
  Pending and failed instances report unavailability; unrelated sessions
  and connections remain usable. At boot the stored instances start
  independently. Subscribers receive `Feed::Instances` whenever offers change.
  Instance commands keep their final durable
  `Accepted` receipt. Current instance state comes only from `Welcome` and
  the live feed; sign-in completion carries no snapshot, and command
  answers return directly to their caller without a duplicate link update.

- **Issue edits and landing recovery**. Issue status and checklist edits
  carry the browser's expected state. An interrupted landing can be
  retried after the host restarts.

- **Worktrees and commit snapshots** (architecture §5.3). Each new session
  gets a managed Git worktree from committed HEAD, an initial snapshot
  and a snapshot per actual turn. The session records each snapshot's
  commit; unchanged turns may share that commit. The phone shows bounded
  immutable diffs between consecutive recorded snapshots, including all
  commits the agent made within a turn. Capture recovery retains pending
  work across failures and restart, and retention refs keep recorded
  commits reachable. Diffs only read: there is no undo or provider
  rewind. Continue resumes the provider conversation on the same host.

- **Single CLI pin source** (#28). `tools/pin` writes only the
  provider's `crates/<provider>/src/pin/pins.rs`, including upstream
  verification notes. Fixture and supported-version checks use the compiled
  pins, in each provider's crate.

- **Codex start errors** (#19). Failed starts and Continue keep the
  app-server’s JSON-RPC error or last nonempty stderr line.
- **Shared adapter types** (#25). Both adapters emit the `agents` tool kinds,
  file changes and outcomes. The daemon keeps request ids, session state and
  approval wording.
- **Agents.** `claude-cli` drives the `claude` CLI (stream-json control
  protocol) and `codex-cli` the `codex app-server` (JSON-RPC): messages,
  streaming deltas, tool calls, approvals, interrupts.
- **Agent core.** Sessions speak the provider-agnostic `agents` API. The
  `claude` and `codex` wire crates map into it and, like `agents`, are pure
  `no_std` cores; process I/O lives in `claude-cli`, `codex-cli` and
  `install`.
- **Host.** `conductord` serves one host over iroh: a durable redb event log
  (streaming deltas are live-only), a session actor per agent, one shared
  Codex app-server per Codex instance. Clients reach it over iroh through
  n0's public relays.
- **The TypeScript app** (ADRs 0001 and 0002, architecture §7 to §9).
  The Dioxus app, `crates/app`, `crates/ui`, `crates/client`, `net`'s
  `HostLink`, the browser e2e and the `headless` example are gone; nothing
  compiles to wasm but `crates/web-iroh`, iroh in the browser as a byte
  pipe to one host through its relay. The client link is JSON (ALPN
  `conductor/45`), each message type naming its codec, with postcard kept
  between hosts (`conductor-fleet/3`); signed fleet payloads are signed
  over their exact JSON bytes and verified before they are parsed. The
  app's Effect Schemas are generated from `link`'s JSON Schema into
  `web/src/proto/` (`tools/schemas.sh`, the `schemas` suite). `web/` is
  Vite, React and Effect: a link per paired host that walks to Live,
  reconnects with a jittered backoff and resubscribes after its cursor;
  the one fold of a host's events; and the thin app: pair hosts by join
  link or ticket and forget one, a session list per host with its link
  state, start a session on a host and instance in a project or directory,
  and a session's live transcript with tool cards, where you send, approve
  or deny, answer, stop, continue and finish. Pages builds `web/` with npm.
  The app's Vitest tests replay recorded host feeds in
  `web/fixtures/feeds/`. The screens in this list that the thin app
  lacks (turn diffs, *Projects*, issues, credentials and sign-ins, fleet
  invites, *Remove*, checklists, Web Push) are Next, item 6; their *Try*
  steps wait for it. *Try:* add the app from Pages to the iPhone's Home
  Screen, pair two hosts and run a session on each
  (`docs/manual-acceptance.md`).
- **Several hosts in one app** (home step 2, architecture §8). The app keeps
  every paired host in local storage, one `conductor.v2.host.<endpoint id
  in hex>` record per host with its ticket and last name, and a live link
  to each at once. Earlier records are set aside, and those hosts are
  paired again. The list groups sessions by host under each one's name and
  link state, *Hosts* pairs by a join link or ticket and forgets a host,
  and a new session picks a host, then one of its instances. Pairing a
  host again replaces its ticket.
- **The home and its projects** (home steps 3 and 4, architecture §5.4).
  The home repository `JimLundin/home` lists each project in
  `projects/<id>/project.toml`, its folder named by the project's id
  (name, title, remote, root commit, profiles), which `home repos` and
  the `AGENTS.md` table now read. A name is unique, and the app shows the
  title; renaming a project is an edit of its file, and it keeps its
  checkouts and sessions. A host
  given `--home <remote>` (stored, so given once) clones it into
  `<data>/home-repo` and pulls it every five minutes, on *Sync* and before
  a read that needs it; offline and failed pulls are its state (`Synced`,
  `Behind`, `Failed`). It finds each project's checkouts by root commit and
  remote in its `--projects <dir>` roots, keeps them in `<data>/home.state`,
  and clones one from the app into `<data>/projects/<project>`. The new
  `home` core crate holds the decisions. `Session::Start` takes a project or
  a directory; the app has a *Projects* view and a
  project picker in the new-session form. *Try:* start a host with `--home
  https://github.com/JimLundin/home.git --projects <your projects dir>`,
  open *Projects*, clone a project you lack, and start a session in it.
- **Removing a session** (home step 5, architecture §5.3). A closed
  session's *Remove* (`Session::Remove`) stops its
  agent between turns, commits what its worktree holds onto
  `conductor/<ulid>`, pushes that branch alone to the checkout's `origin`
  without force, and removes the worktree. The branch stays in the checkout;
  snapshot retention refs keep commits from surviving events reachable.
  `SessionRemoved` says whether it was
  pushed. The removed session leaves the app and takes no command. *Try:*
  close a session, press *Remove*, and find its work on
  `conductor/<ulid>` in the checkout.
- **Issues** (home step 6, architecture §5.4). Each project in *Projects*
  lists its issues by status, with a field that opens one; an issue's pane
  sets its status and priority, edits its title, body and labels, checks
  off its checklist and takes notes, which the host dates and signs with
  its name and the device's. The host writes each
  change to `projects/<project>/issues/<id>.md` in its clone of the home,
  one commit each, and pushes; when the remote has moved on it resets to
  it and applies its kept changes again on top, so concurrent notes both
  stay. Offline changes wait in `<data>/home-edits.state` and land on the
  next sync. *Try:* open an issue in a project, add a note from the
  phone, and find the commit in the home's history.
- **Sessions on issues** (home step 7a, architecture §5.4). *Start a
  session* on an issue's pane opens the new-session form on it: the
  session works on a branch cut from the issue's `issue/<id>`, which the
  host takes from the project's remote when it has it; its first message
  is the issue's brief, and the issue moves to `in-progress`. One session
  per instance and issue. *Try:* start a session on
  an issue and read its first message.
- **Finishing a session** (home step 7b, architecture §5.4). *Finish* on a
  session on an issue asks its agent for a summary, stops it, lands its
  branch on the issue's (fast-forward, or a merge commit; a conflict
  lands nothing), tries to push to the project remote, adds the summary
  to the issue's notes and moves the issue to `review` when it gained
  commits. The transcript's **Finished** row shows
  whether the branch was pushed or kept on the source host. *Try:* finish
  an issue session, verify the push and start one on another host with a
  checkout that can fetch the issue branch ([home](home.md#4-flows)).
- **Turn commits when finishing** (home step 7c, architecture §5.3). Each
  turn records the session branch's tip after capturing changes on top
  of any commits the agent made. A turn with no further file changes
  needs no new commit. Finishing lands the accumulated branch commits on
  the issue branch. *Try:* run two turns, inspect each turn's diff and
  `git log` on `conductor/<ulid>`, then finish the session and inspect the
  issue branch.
- **Instructions from the home** (home step 8, §5). Each start of a
  session's agent is told the home's shared rules, where its project's
  profiles keep their checks on the host, and the project's `AGENTS.md`
  when its CLI wouldn't read it: an appended system prompt for Claude,
  developer instructions for Codex. No protocol change. *Try:* start a
  Claude session in a project with only an `AGENTS.md` and ask it what its
  standards are.
- **Checklists in their sessions** (home step 9). Agent and sub-agent
  checklists show in their sessions and make no issue notes; the summary
  turn and explicit notes are what reach the issue. The home's build order
  (home §8) is done. *Try:* start a session on an issue, let its agent
  tick off a step, then finish it and read the summary note.

- **A wider session model** (§3). Questions to the user (Claude's
  `AskUserQuestion`, Codex's `requestUserInput`) as a card with options
  and typed text; approve-for-session as a third button that names what it
  allows; reasoning summaries as a streaming "Thinking" disclosure; a
  model and an effort per session from what each CLI lists, the effort
  changeable between turns from the header; and `Continue`, which starts a
  closed session's CLI again with the provider's resume in the private
  home. Claude's reasoning text needs experimental betas, which the
  owner's gateway refuses, so through it Claude shows none (Claude
  `PROTOCOL.md`). *Try:* restart a session's CLI and continue it.
- **Sign in from the app** (#12, §6.3). Gateway credentials, direct
  Anthropic and OpenAI API keys and a Claude setup token from the
  credentials screen, encrypted in the host's credential store (macOS
  Keychain, Windows Credential Manager, Linux Secret Service, or a private
  file; [backup and recovery](credential-store.md)). An instance's
  subscription sign-in runs the CLI's own sign-in in the instance's home
  (`claude auth login --claudeai`, whose page gives a code to paste back;
  `codex login --device-auth`), described by each provider's core as data
  (`agents::SignIn`) and followed live as `Feed::SignIn`, stored nowhere.
  A different account takes over the private home and work directory
  through a journaled roster update. *Try:* on a fresh host, sign in to
  Claude and Codex from the iPhone without touching the host's keyboard.
- **Durability** (#11). redb and postcard store the event log, command
  receipts, open requests and conversation ids; the host key, ticket token
  and epoch persist, and clients resume their cursors after a restart.
  Open requests are withdrawn and in-flight turns interrupted on restart,
  and `Continue` resumes the native conversation. Session ids are
  fleet-unique ULIDs, with `CLAUDE_CODE_PROJECT_DIR_NAME=<ulid>` isolating
  Claude transcripts. External CLI actions sent before a crash can still
  lack a receipt (architecture §5.3). *Try:* restart the host mid-session
  and continue from the phone.
- **Fleet and pairing** (#14, §6.1, §6.4 to §6.6). A fleet root key signs
  device and host certificates; `conductord join` and *Add host*; a signed
  fleet config with the roster, API keys and the Claude token, served by
  any host; a per-host Codex ChatGPT sign-in after a join. *Try:* add a
  second host from the phone and run a Claude session on it with the
  fleet's token, without signing in again.
- **Push** (#15). Web Push from the host when an approval or question
  opens. The app's service worker went with the Dioxus app, so nothing
  subscribes until Next, item 6.
- **Subagents** (#31, home §6). Child session events record each
  subagent's lifecycle, type, result, transcript, requests and checklist;
  the phone shows the child beside the tool call that started it. *Try:*
  ask a Claude session to delegate two pieces of work and watch each
  child's progress and result.

## Next

Each item ends in something to try. The fleet design is
`docs/architecture.md` §6.

1. **Decisions out of the shell.** The shell crates still make about a
   quarter of the decisions, and `exceptions.toml` lists each shell file
   that does as debt. Move the registry, serve, log and workspace
   decisions into `host`, retiring an entry with each. *Try:* `tools/check.sh
   commit` passes with that entry gone from `exceptions.toml`.
2. **Instance settings from the app.** Per-instance model and CLI
   settings; the store already keeps both. *Try:* change a gateway
   instance's default model from the phone and start a session on it.
3. **One place to list a provider.** `Provider` is the name each plugin
   declares and each provider pins its own CLI, so only `conductord` lists
   the providers. *Try:* a test plugin registered only in `conductord` runs
   a session.
4. **Live checks still owed.** Experiments E2 (one Claude setup token on
   two hosts) and E3 (a second ChatGPT device-code sign-in;
   `docs/verification/codex_manual_probe.py`), architecture §6.9; live
   Codex subagent events and child approvals; token revocation and native
   keystore verification (#12). *Try:* run E3's probes in a disposable
   `CODEX_HOME` and record the result in §6.9.
5. **Every session has a workspace.** Every session started now records
   `WorkspaceCreated` before anything else, but the host session, its
   `Book` and Continue still handle a logged session without one by running
   in its original directory (daemon e2e
   `a_session_without_a_worktree_continues_in_its_original_directory`).
   Old logs are disposable, so refuse such a session at restore and make
   the workspace required. *Try:* the host tests build every session with
   its workspace, and that e2e test is gone.
6. **The rest of the app.** The thin app lacks the Dioxus app's turn
   diffs, *Projects* and issues, credentials and sign-ins, fleet invites
   and browser device identity, *Remove*, checklists and Web Push; the host
   serves them all already. Bring each back in `web/` as its own spec,
   after T3 Code's designs, with its check in `docs/manual-acceptance.md`.
   *Try:* sign an instance in from the iPhone's Home Screen app.

## Known bugs

- A fresh connection sometimes gets no `Welcome` (#6). The client now
  closes a stalled Hello exchange after 10 s and retries with backoff;
  handshake timing logs distinguish transport from host processing waits.
  Loopback regressions cover a missing reply and a delayed successful reply.
  Three fresh Chromium connections through the public relay on 2026-11-14
  reached Live in 1.30–1.59 s without running either provider. The original
  intermittent failure was not reproduced and its cause remains unresolved;
  this is a timeout mitigation, not a root-cause fix. The separate registry
  blocking path in #23 is fixed; it was not proven to cause this failure.

Open issues are tracked at https://github.com/JimLundin/conductor/issues.

## Known risks

- n0's free relays now carry all traffic. If n0 ever restricts them, use a
  paid n0 relay or iroh's open-source relay on a small server. The relay URL
  is one setting, so no code changes.
