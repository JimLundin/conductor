# Roadmap

What is built and what comes next. Update it in the commit that changes it.

## Built

- **The shared Rust standard.** The subset is retired; `tools/check.sh`
  runs the standard's checks. The core crates are `proto`, `client`,
  `agents`, `claude`, `codex` and `host`; `net`, `daemon`, `conductord`,
  `runtime`, `claude-cli`, `codex-cli`, `install` and `app` declare
  themselves the shell.
  `client` holds the host link's state machine, and `host` holds
  checkpoints and tasks. Next: the shell still makes most decisions, and
  `exceptions.toml` lists each shell file that does as debt. Move the
  registry, serve, log and workspace decisions into `host`, retiring an
  entry with each.

- **Providers as plugins** (§5.2, §6.3). The daemon library takes a map of
  `runtime::Plugin`s and names no provider: each is its `agents::Profile`
  as data (CLI pin, probes, login, home config, label, note and credential
  forms) plus its starter, which takes the stored credentials over the
  runtime protocol. Credentials are generic form values the app draws
  from `Welcome.catalog`, checked by their form and read by each
  provider's core (ALPN `conductor/13`, credential store format 3). The
  binary moved to `crates/conductord`, the one place that wires the
  providers, with `--cli <provider>=<path>`, `--model <provider>=<model>`
  and `login` (now per instance, below). Next: the pins in `agents::pin` and `Provider`
  itself are the last places a new provider is listed.

- **Instances** (§5.2, §6.2, §6.3). A host runs named instances of each
  provider, such as "Claude (API)" through a gateway beside "Claude
  (subscription)" on its own sign-in. Each instance has its own credential,
  its own private home `<data>/home/<instance>`, its own work dir and its
  own agents, so a Codex instance gets its own app-server. Sessions start
  on an instance and record it, so Continue and undo stay on it. The app's
  credentials screen lists the instances, adds one per provider, renames
  and removes them (refused while a session runs on one), and the
  new-session picker lists instances. A fresh host gets one instance per
  provider, named after it; `--cli` and `--model` are each provider's
  defaults, and `conductord login <instance>` signs one in on the host
  (ALPN `conductor/14`, credential store format 4, log format 2). Next:
  sign-in from the app (item 3), and per-instance model and CLI settings
  from the app; the store already keeps both.

- **One agent runtime** (§5.2). The daemon asks one interface for agents:
  `agents::runtime`'s commands and events over a `runtime::Runtime`
  channel pair, with Claude and Codex as two plugins behind it. Each
  provider's core translates the protocol into its wire as a state
  machine, Claude's resume fallbacks included, and `host::session` and the
  daemon's session know no provider. Logging moved out of the cores into
  the shell, and the shell code bent to the old complexity measure is
  plain `async fn` and `select!` again.

- **Managed CLIs** (#1) (§6.2). `conductord` installs the pinned Claude
  Code and Codex into its data dir, checks their hashes and runs only those,
  with private `CLAUDE_CONFIG_DIR` and `CODEX_HOME` and updates off. Gateway
  credentials run them (`tools/live-host.sh --register`). Subscription
  sign-in and its check on a host without the CLIs are postponed to #12;
  `conductord login <instance>` remains as the host-terminal fallback.

- **Responsive provider refresh** (#23). Credential saves, model discovery
  and Codex reconfiguration no longer hold the registry or the global
  command transaction while a CLI responds. Refreshes are serialized per
  instance, coalesce superseded configurations and discard stale results.
  Pending and failed instances report unavailability; unrelated sessions
  and connections remain usable. Credential retries retain their final
  durable `Instances` receipt.

- **Worktrees and turn undo** (#13). Each new session gets a managed Git
  worktree from committed HEAD, an initial checkpoint and a snapshot per
  actual turn. The phone shows bounded immutable diffs and can undo files
  and conversation history to before a selected turn. Durable native-first
  phases support retry after failure or restart; legacy sessions continue
  in their original directory. Claude uses UUID prefix forks, Codex uses
  verified `thread/revert` prefixes.

- **Single CLI pin source** (#28). `tools/pin` writes only
  `crates/agents/src/pin/pins.rs`, including upstream verification notes.
  Fixture and supported-version checks use the compiled pins.

- **Codex start errors** (#19). Failed starts and Continue keep the
  app-server’s JSON-RPC error or last nonempty stderr line.
- **Shared adapter types** (#25). Both adapters emit the `proto` tool kinds,
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
- **Client.** `HostLink` per host: reconnect with backoff, a hedged dial,
  catch-up from the cursor. `HostView` folds events into what the UI shows.
- **App.** One Dioxus web app, installed as an app on the desktop and the
  iPhone: session list, new session, session pane, phone layout. Browser e2e
  on the real daemon with fake CLIs.

## Next

Each item ends in something to try. The fleet design is
`docs/architecture.md` §6.

2. **A wider session model** (§3). Built in round A, on
   `session-questions`: questions to the user (Claude's `AskUserQuestion`,
   Codex's `requestUserInput`) as a question card with options and typed
   text, and approve-for-session as a third button that names what it
   allows. Built in round B, on `session-model-b`: reasoning summaries as a
   "Thinking" disclosure that streams live; a model and an effort picked per
   session from what each CLI lists, shown in the session's header; and
   `Continue`, which starts a closed session's CLI again with the
   provider's resume in the private home. Claude's reasoning text needs
   experimental betas on, which the owner's gateway refuses, so through it
   Claude shows none (Claude `PROTOCOL.md`). Continue covers CLI and host restarts, with the durable log and native
   conversation ids from item 4. Built in round C, on `session-model-c`:
   the agent's checklist (Claude's `TodoWrite`, Codex's plan) kept by the
   host as its own tasks, each with a stable id and the session that holds
   it, shown above the transcript; and the effort changed between turns
   from the session's header, which the next turn takes (Claude in its
   running process, Codex on that `turn/start`). Both CLIs need a switch for
   the checklist tool (§5.2). *Try:* restart a session's CLI and continue it
   (`tools/live-host.sh` checks it as `continue=pass`); ask an agent to plan
   a change as a checklist and watch it tick off on the phone, then lower
   the effort from the header before the next message.
3. **Sign in from the app** (#12) (§6.3), on one host. Built first, on
   `credentials`: gateway credentials (Claude's `ANTHROPIC_BASE_URL` and
   `ANTHROPIC_AUTH_TOKEN`, a Codex custom model provider) set from the
   app's credentials screen and stored encrypted under `<data>/store.key`;
   `tools/live-host.sh --register` runs the pinned CLIs on them. Next: API
   keys, the Claude `setup-token` token and the Codex device-code sign-in,
   entered on the phone, and the platform keystores for the host key.
   `Welcome` reports each instance's sign-in kind. A sign-in from the app
   runs per instance, in the instance's home, as `conductord login
   <instance>` does on the host today. *Try:* on a fresh host, sign in to
   Claude and Codex from the iPhone without touching the host's keyboard.
4. **Durability.** (#11) Built on `issue-11-durability`: redb + postcard
   store the event log, command receipts, open requests and conversation ids.
   The host key, ticket token and epoch persist; clients resume their cursors
   after a host restart. Open requests are withdrawn and in-flight turns
   interrupted on restart, and `Continue` resumes the native conversation.
   Session ids are fleet-unique ULIDs, with
   `CLAUDE_CODE_PROJECT_DIR_NAME=<ulid>` isolating Claude transcripts.
   Receipts and durable command effects commit together; external CLI actions
   sent before a crash can still lack a receipt (architecture §5.3).
   *Try:* restart the host mid-session and continue from the phone.
6. **Fleet and pairing** (#14) (§6.1, §6.4 to §6.6). A fleet root key signs
   device and host certificates and replaces the shared ticket token;
   `conductord join` and *Add host*; a signed fleet config with the roster,
   pins, settings, API keys and the Claude token, served by any host; a
   per-host Codex ChatGPT sign-in after a join; several hosts in one
   sidebar. *Try:* add a second host from the phone and run a Claude
   session on it with the fleet's token, without signing in again.
7. **Push.** (#15) Web Push from the host when an approval or question opens.
   *Try:* lock the phone and get the approval notification.
8. **Move a session** (#16) (§6.8), after experiments E4 and E5 (§6.9). Bundle
   the branch, copy the transcript and the events, resume on the target.
   *Try:* move a session from WSL to the Mac between turns and continue it
   from the phone.
9. **Hand tasks between sessions** (#30). It builds on the task model of
   item 2: tasks are the host's own, with a `TaskId` that survives every
   checklist update, `Continue` and a host restart, and a `holder` that
   names the session working on each. Next: how an agent receives or
   claims a task, and how a task leaves one session or agent for another.
   *Try:* hand a task from a Claude session to a Codex session and watch it
   leave one checklist and join the other.
10. **Monitor the subagents** (#31) each session runs: Claude's `Agent`
    tool, whose subagent frames carry `parent_tool_use_id` (Claude
    `PROTOCOL.md`), and Codex's collab and spawn-agent items, shown in the
    app. Round C keeps a subagent's checklist out of its session's tasks.
    *Try:* watch a session's subagents work, and finish, from the phone.

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
