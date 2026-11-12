# Conductor architecture

How Conductor is built today, and the designs for what comes next. What is
built and the order of the next slices are in `docs/roadmap.md`. The wire
types are documented in `crates/proto/src/lib.rs`, which is the source of
truth for the protocol; this page explains the shape around them.

## 1. What it is

One app that runs and supervises **Claude Code** and **Codex** sessions on
several of your machines, with no server of ours in the path.

- **Hosts** run `conductord`, which drives the `claude` and `codex` CLIs in
  their native protocols and keeps each host's event log.
- **The client is one web app** (Dioxus compiled to WebAssembly), served as
  static files from GitHub Pages and installed as an app: Chrome, Edge or
  Safari's Add to Dock on the desktop, Add to Home Screen on the iPhone.
  There is no native client and no Apple Developer account.
- **Traffic is end-to-end encrypted.** Clients reach hosts over iroh through
  n0's public relays (§4).

### Goals for v1

- Run Claude Code and Codex sessions on any paired host, one git worktree per
  session.
- Show every host in one interface, on the desktop and on the iPhone.
- Handle approvals, questions, interrupts, resume and per-turn diffs from any
  device.
- Catch up correctly after a disconnect, such as a phone waking up.
- Rust everywhere. The only handwritten JavaScript is the service worker Web
  Push requires.

### Not in v1

A terminal emulator, agents other than Claude Code and Codex, a planner or
orchestrator agent, kanban, CI and review routing, an embedded browser
preview, cloud workers, multi-user use.

## 2. Decisions

| # | Decision | Why |
|---|---|---|
| D1 | Rust for the daemon, the client core and the UI | One set of types end to end. The agents' SDKs are thin wrappers over stdio protocols we speak directly. |
| D2 | One Dioxus web app for every device | Dioxus renders real HTML, which mobile Safari needs for keyboard, scrolling and selection. An installed web app behaves like a native one on the desktop and the iPhone, so there is one build and one test target. A thin native shell can come later if a need appears (tray, global hotkey). |
| D3 | iroh 1.x through n0's public relays; the relay map is one setting | Connections are authenticated and encrypted and dialed by public key. A browser can't send UDP, so in a browser every iroh connection is relayed. |
| D4 | Each host is the only source of truth for its own sessions | No replication between hosts. Clients connect to each host and merge. Catching up is "events after seq N". |
| D5 | Trust comes from a fleet root key that signs device certificates (planned, §6) | Hosts store only the root's public key, so pairing a device once gives it access to every host. |
| D6 | Speak each agent's native protocol, with owned wire types for only the messages we use | Claude's stream-json control protocol and Codex's app-server JSON-RPC carry approvals, interrupts and resume. Lowest-common-denominator protocols (ACP) lose them. |
| D7 | Shell out to the `git` CLI | Worktrees, hooks and the user's git config behave exactly as on the command line. |
| D8 | Length-prefixed postcard frames over bidirectional streams | Compact, and our own Rust types on both ends. postcard isn't self-describing, so every change to a wire type bumps `ALPN`. |
| D9 | Hosts on Linux/WSL, macOS and Windows; development in WSL | The developer works in WSL. |

## 3. The model: one session shape for both agents

The daemon translates each provider's protocol into one provider-neutral
model (`crates/daemon/src/mapping.rs`). The app only ever sees that model,
so one UI and one set of commands work for both agents.

- **Commands:** `StartSession { provider, directory, model, effort }`,
  `SendMessage`, `Interrupt`, `Continue`, `Resolve { request, Approve |
  ApproveForSession | Deny }`, `Answer { request, answers }`. Each command
  gets one `CommandResult`. A reply that doesn't fit its request (a
  decision for a question, the wrong number of answers, `ApproveForSession`
  where none is offered) is refused as `WrongReply`.
- **Events:** a session is created, becomes ready, closes, and becomes
  ready again after a `Continue`; a user message; the agent's reasoning and
  the assistant's text, each streamed as deltas and then as the finished
  text; tool calls (`Command`, `FileChange`, `Other`) started and finished;
  approval requests opened and resolved (first decision wins, and a late
  one is told who decided); questions asked and answered; a turn
  completes, is interrupted or fails.
- **Model and effort:** `Welcome` lists, for each provider the host can
  run, the models its CLI offered when the host started (`Models`: each
  model's id, name, description and efforts, and the default).
  `StartSession` may name one of them and one of its efforts (`low` to
  `ultra`); a model or an effort the host doesn't list is refused as
  `NotOffered`. With none named, the session gets the default: a Codex provider
  credential's model, which is what its gateway serves; else the host's
  `--claude-model` or `--codex-model`; else the one the CLI marks as its
  default. The list comes from the CLI even with a credential. `SessionCreated` carries the model and effort
  the session runs with. The effort is fixed for the session.
- **Reasoning:** `ReasoningDelta` streams and `Reasoning` stores the
  finished text, which is the provider's summary of the model's reasoning:
  a few sentences per stretch, never the raw thoughts, so the host stores
  it whole. Each stretch is its own item; the app shows it as a collapsed
  "Thinking" disclosure, whose summary reads "Thinking…" while it streams.
  A stretch without text (Claude through a gateway that omits it) shows
  nothing.
- **Continue:** a session outlives its agent's process. When the process
  exits (a crash, a kill, a stopped Codex app-server), the session closes
  and keeps the id of its conversation. `Continue` starts the CLI again on
  that conversation with the provider's resume (Claude's `--resume <id>` in
  the private home, Codex's `thread/resume` on a new app-server), with the
  same model and effort and the provider's current credential, and logs
  `SessionReady`. The session keeps its id and its log. `Continue` is
  idempotent: on a session whose agent runs it answers `Accepted` and does
  nothing, so a double tap or a second phone ends in the same state. A session closed
  before its first turn starts its CLI afresh, since there is nothing to
  resume yet. Continue covers CLI restarts within a running host only: a
  host restart loses the in-memory log and the session's conversation id
  until durability (`docs/roadmap.md` item 4).
- **Requests:** an approval and a question are both requests: one
  `RequestId` space per session, first reply wins, and the agent's
  withdrawal, the end of the turn or the end of the session withdraws one
  still open. `RequestOpened` carries `for_session`, what
  `ApproveForSession` would allow in the agent's words (Claude's rule
  `Bash(git status)`, Codex's command), or nothing if it offers none.
  `QuestionAsked { request, questions }` carries each question's header,
  text, options (label and description) and whether one or many may be
  chosen. Every question also takes typed text, which both agents accept.
  `Answer` carries one answer per question, in order: the chosen labels,
  then any typed text. It resolves as `Answered { answers, by }`.
- **Durable and live:** every event except the streaming deltas (assistant
  and reasoning) is durable:
  it takes the next `HostSeq` and is replayed on catch-up. Deltas go only to
  subscribers that are live, because the finished message replaces them. A
  delta carries the seq of the last stored event, so the client's gap check
  still holds. A client that reconnects mid-reply may miss deltas; the
  finished message repairs the text.

What both providers have belongs in the model. What only one has goes in
typed per-provider options instead of being forced into it: Claude's
permission modes, Codex's sandbox. Raw provider JSON never reaches the UI.

How each agent carries these, from the `s1` and `s2` recordings:

| | Claude Code 2.1.316 | Codex 0.166 |
|---|---|---|
| Question | `can_use_tool` for `AskUserQuestion`; the questions are in `input` | `item/tool/requestUserInput`, with question ids; needs the `default_mode_request_user_input` feature outside plan mode |
| Answer | `allow` with `updatedInput` = the input plus `answers`, question text to answer, labels joined with ", " | RPC result `{answers: {<id>: {answers: [...]}}}` |
| Withdrawn | `control_cancel_request` | `serverRequest/resolved` after the interrupted turn ends |
| For the session | `allow` with `updatedPermissions`: the suggested allow `addRules`, destination `session`; with none suggested, not offered | `acceptForSession` on a command or a permissions request, taken though `availableDecisions` omits it (`s2` step 8 checks it); not offered on a file change, which no run has recorded |
| Refused | Duplicate question texts, by the CLI before it asks | A question with `isSecret`, by the host, which logs and replays every answer |
| Models | The `initialize` response's `models`, with `supportedEffortLevels`; a `default` row | `model/list`, with `supportedReasoningEfforts` and `isDefault` |
| Effort | `--effort`; `get_settings` reports it back | `model_reasoning_effort` in the thread's `config`; `thread/start` reports it back |
| Reasoning | `thinking_delta`s and a `thinking` block; text only with `--thinking-display summarized` and experimental betas on | `item/reasoning/summaryTextDelta` and the item's `summary`, with `model_reasoning_summary` set |
| Resume | `--resume <session_id>` in a new process | `thread/resume { threadId }` on a new app-server |

The `AskUserQuestion` tool call is not shown as a tool call; its question
card is.

Next in the model (`docs/roadmap.md`): plans, and the effort changed
between turns.

## 4. Transport

### 4.1 Streams

ALPN is `conductor/<n>`. Every bi-stream starts with one `Open` frame from the
client:

| Stream | Purpose |
|---|---|
| `Open::Hello` (first) | Authenticates and returns `Welcome { host, epoch, head_seq, providers }`. |
| `Open::Command` | One `Command` → one `CommandResult`, then the stream closes. |
| `Open::Subscribe { epoch, after_seq }` | Replays the log after the cursor, then streams live. A cursor from another epoch is refused. |

Each frame is a `u32` length and a postcard body.

### 4.2 iroh

- `Endpoint::builder(presets::N0)` with a custom relay map: n0's relays named
  without the root dot, because WebKit refuses the dotted name's certificate
  (`proto::without_root_dot`, `client::platform::n0_relays`).
- The host waits for `Endpoint::online()` and prints a ticket with its node id,
  home relay and the auth token (`proto::Ticket`), as a QR code and a
  `#join=` link. The fragment never reaches the static server.
- A dial that stalls for 1.5 s is hedged with a second dial on a fresh
  endpoint.
- In the browser, iroh needs the `tls-ring` feature, so building the web app
  needs clang for wasm32.
- Tests use `presets::Minimal` with relays disabled, on loopback.

There is no WebRTC upgrade: iroh over n0's relay carries all traffic, and
that is enough for this app's text-sized traffic.

## 5. The host (`conductord`)

### 5.1 Actors

Every piece of state has one owner task; handles talk to it over channels.

- **Log** assigns each durable event its seq, keeps the events, and hands a
  new subscriber its replay and the live broadcast in one turn, so nothing
  falls between them. A subscriber that falls behind the broadcast is dropped
  and resubscribes from its cursor.
- **Registry** maps session ids to session handles and starts sessions.
- **Session** owns one agent, its turn and its open requests. An interrupt
  that arrives before the agent began the turn is held until it does. An
  open request is withdrawn when the turn ends. It outlives the agent's
  process: once that exits it takes only `Continue`, which starts the agent
  again on the same conversation.
- **CodexHub** owns the one `codex app-server` process per host and routes
  its threads to their sessions.
- **Serve** accepts connections, checks `Hello`, dispatches commands and
  feeds subscriptions.

### 5.2 Agents (`crates/agents`)

**Claude Code:** one long-lived process per session, started as

```
claude --input-format stream-json --output-format stream-json --verbose \
       --permission-prompt-tool stdio --include-partial-messages \
       --thinking-display summarized --permission-mode <mode> \
       [--model M] [--effort E] [--resume <session_id> | --session-id <uuid>] \
       --setting-sources user,project,local
```

with `CLAUDE_CONFIG_DIR` set to the host's private home (§6.2).

- stdout is JSON lines: `system`/`init` (once per turn, with the
  `session_id`), `assistant` and `user` messages with content blocks,
  `stream_event` partials and `result`.
- The control protocol runs both ways as `control_request` /
  `control_response`. We send `initialize`, `interrupt` and
  `set_permission_mode`; the CLI sends `can_use_tool`, answered with
  `allow` (echoing the input as `updatedInput`) or `deny`. Every control
  request we don't serve gets an error reply, or the CLI waits forever.
- An interrupted turn reports `error_during_execution`; only
  `terminal_reason` (`aborted_streaming`, `aborted_tools`) says it was an
  interrupt.
- The `initialize` response's `claude_code_version` is checked against the
  tested range; new CLI versions are re-recorded with `s1`.

**Codex:** one `codex app-server` per host serves every thread.

- Newline-delimited JSON-RPC with no `jsonrpc` field; allow very long lines.
  The handshake is `initialize` with `capabilities: {experimentalApi: true}`.
- We call `thread/start`, `thread/resume`, `turn/start`, `turn/interrupt`
  and `model/list` (once, when the host starts), and handle
  `turn/started`, `turn/completed`, `item/started`, `item/completed`,
  `item/agentMessage/delta` and `item/reasoning/summaryTextDelta`.
- Approvals arrive as server requests (`item/commandExecution/requestApproval`,
  `item/fileChange/requestApproval`, `item/permissions/requestApproval`,
  `item/tool/requestUserInput`) and `serverRequest/resolved` withdraws one.
  Decisions are `accept`, `acceptForSession`, `decline`, `cancel`; keep
  Codex's own option values, since a label is not always a valid reply.
- Sessions use Paseo's preset: `on-request` approvals with a
  `workspace-write` sandbox.
- Traps (from T3 Code's provider notes): `/goal` can start follow-up turns
  on its own. Their other note, that questions are answered with a new
  user message, does not hold for 0.166's `requestUserInput`, which takes
  an RPC result even when `isBlocking` is `false` (Codex `PROTOCOL.md`).
- Every thread starts with the `default_mode_request_user_input` feature
  on, so the model can ask questions outside plan mode. Codex marks it
  under development and sends a `warning` for it. Every thread also sets
  `model_reasoning_summary = "auto"`: with a custom model provider Codex
  sends no reasoning summary otherwise.

**Verification:** replay tests feed recorded stdio from `fixtures/` through
the parsers and assert the exact events. `cargo run --bin s1` and `s2` drive
the real CLIs and re-record the fixtures; run them after changing an adapter
or upgrading a CLI.

### 5.3 Planned: durability, worktrees, checkpoints

- **Store:** redb tables, postcard values: `sessions`, `events` (the log),
  `raw_events` (provider wire, to replay through a fixed mapping),
  `pending_requests`, `command_receipts`, `roster`, `push_tokens`. A cursor
  is `(epoch, seq)`; the epoch is created with the database, so a wiped one
  is detected and the client starts over.
- **Idempotent commands:** a `CommandId`'s receipt is stored with its
  effects, so a retry returns the stored result (T3 Code's
  `CommandReceiptStore`).
- **Restart:** child processes die with the daemon. On start, in-flight
  turns are marked interrupted and resumed natively (`--resume`,
  `thread/resume`) on the next message.
- **Worktrees and checkpoints:** one worktree per session under
  `<data>/worktrees/<project>/<session>`. At the end of each turn, snapshot it
  without touching the user's index (`GIT_INDEX_FILE=<tmp> git add -A`,
  `git write-tree`, `git commit-tree`, `git update-ref
  refs/conductor/<session>/<turn>`). Diffs are `git diff` between refs;
  revert is `git restore --source=<ref>` plus Claude's `rewind_files` or
  Codex's `thread/rollback`. This is T3 Code's mechanism (`GitVcsDriver.ts`).

## 6. Planned: the fleet

The goal is one setup running in several places. Every host runs the same
pinned Claude Code and Codex, installed and owned by `conductord`, with the
same settings and the same credentials, except a Codex ChatGPT login,
which each host signs in to once (§6.6). A
host that joins the fleet can run sessions without a manual setup, and a
session can move from one host to another.

D4 still holds. A session lives on one host at a time and its events are in
that host's log. What the fleet shares is configuration, not sessions.

Facts about the CLIs and their terms are marked **verified** (read in the
cited source, or observed on the developer's machine) or **inferred**. They
were checked on 2026-11-12, against Claude Code 2.1.316 to 2.1.322 and
Codex 0.166 to 0.167.

### 6.1 Trust and pairing

Today a ticket's random token stands in for a device certificate. The fleet
replaces it.

- A **fleet root key** (Ed25519) signs `DeviceCert { fleet, device, name,
  role }`. The role is `Owner` for a browser and `Host` for a `conductord`.
  Hosts store only the root's public key and check the certificate in
  `Hello` against it, the authenticated iroh node id and the revocation
  list in the fleet config (§6.5).
- Hosts talk to each other with the same certificates, over iroh, on their
  own ALPN `conductor-fleet/<n>`. Clients never open it.
- The root key lives on one host, the **root host**. It is the first host,
  which creates the fleet with `conductord fleet init`. It signs device
  certificates and every fleet config version, and the other hosts hold no
  signing key. `fleet init` also writes the root key once as a
  passphrase-sealed file for the owner to keep, so a lost root host doesn't
  strand the fleet (F1, §6.10).
- A browser keeps its device key as a non-extractable WebCrypto key in
  IndexedDB, and iOS gives a Home Screen app its own storage, so the phone
  pairs from the installed app. The app shows its public key as a QR code
  and `conductord pair` on the root host signs it. A paired app can approve
  a new one the same way.
- Whoever controls the GitHub Pages deploy controls the code the browser
  runs, and from §6.3 on that code carries credentials in flight. Protect
  the deploy branch, use Subresource Integrity, and keep 2FA on.

### 6.2 The daemon manages its own CLIs

`conductord` installs a pinned version of each CLI into its data dir, checks
it against a recorded hash, and runs only that copy. The user's own `claude`
and `codex`, their versions and their logins are never used or touched.

#### Where the binaries come from

Each CLI has several official channels. Conductor downloads the same
per-platform binary the official installers fetch, straight from the
vendor's release location, so a host needs neither Node.js nor a package
manager.

| | Claude Code | Codex |
|---|---|---|
| Official channels | Native installer (`install.sh`, `install.ps1`, `install.cmd`), npm `@anthropic-ai/claude-code`, Homebrew, WinGet, apt, dnf, apk ([setup], verified) | npm `@openai/codex`, a Homebrew cask, GitHub release binaries ([install.md], [releases], verified) |
| Pinning | The installer takes a version (`bash -s 2.1.89`, [setup], verified), but `install.sh` always downloads the newest build first and has it install the requested one (verified, read in the script). npm pins with `@<version>`. | Release tags `rust-vX.Y.Z` pin exactly; npm pins with `@<version>` ([releases], [npm-codex], verified). The cask can't install an older version (inferred). |
| What Conductor fetches | `https://downloads.claude.ai/claude-code-releases/<version>/<platform>/claude`, the URL `install.sh` uses, with `manifest.json` and `manifest.json.sig` from the same directory (verified, read in the script and fetched) | The release asset `codex-package-<triple>.tar.gz`, on Windows too, from `github.com/openai/codex/releases/tag/rust-v<version>`: the package Codex's own `install.sh` installs, with `bin/codex`, `bin/codex-exec-server`, `bin/codex-code-mode-host`, `codex-resources/bwrap` and `codex-path/rg`. The bare `codex-<triple>.tar.gz` holds only `codex`, and without the rest every command an agent runs fails to start (`install.sh` read, both archives listed, and `s2` failing 5 of 9 steps on the bare binary, verified) |
| Platforms | `darwin-arm64`, `darwin-x64`, `linux-arm64`, `linux-x64`, their `-musl` variants, `win32-x64`, `win32-arm64` (the 2.1.316 manifest, verified) | `x86_64`/`aarch64` for `unknown-linux-musl`, `apple-darwin` and `pc-windows-msvc` ([releases], verified) |
| Upstream integrity | The manifest lists a SHA-256 per platform and is signed with Anthropic's GPG key, fingerprint `31DD DE24 DDFA B679 F42D 7BD2 BAA9 29FF 1A7E CACE`, for releases from 2.1.89 on. macOS binaries are signed and notarized, Windows binaries are Authenticode-signed, Linux binaries are not individually signed ([setup], verified). | GitHub publishes a SHA-256 digest per asset ([releases], verified). Linux binaries are cosign-signed through Sigstore (`.sigstore` bundles), macOS binaries are codesigned, Windows binaries are signed with Azure Trusted Signing (the release workflows in [codex-src], verified). `codex-package_SHA256SUMS` lists each `codex-package-*` tarball, and each Linux package has a `.tar.gz.sigstore` bundle over the tarball itself. GitHub artifact attestations return 404 for the CLI assets (verified). |
| Updates | Native installs update themselves in the background. `DISABLE_AUTOUPDATER=1` stops the check; `DISABLE_UPDATES=1` also blocks `claude update` and `claude install`, and is meant for distributing Claude Code "through your own channels" ([setup], [env-vars], verified). | The TUI checks GitHub for a newer release and suggests an upgrade command; it doesn't update itself. `check_for_update_on_startup = false` turns the check off. No update check was found in the app-server (`codex-rs/tui/src/updates.rs`, `config_toml.rs` in [codex-src], verified; the app-server part inferred from a search). |

npm was rejected as the channel. It needs Node.js on every host, Codex's
npm package is a launcher plus a 458 MB platform package ([npm-codex],
verified), and Claude Code's package has registry signatures but no
provenance attestation ([npm-claude], verified). The native installer was
rejected because it always runs the newest build before installing the
pinned one.

#### How a pin is verified

A pin is a version plus one SHA-256 per platform. The hashes are recorded
once, by the developer, with upstream's own checks, and every host checks
against the recorded hash.

1. `tools/pin claude <version>` downloads `manifest.json` and its
   signature, verifies the signature against the Anthropic key fingerprint
   above, downloads every platform's binary and checks each against the
   manifest. `tools/pin codex <version>` downloads every platform's package,
   checks each against GitHub's published digest and
   `codex-package_SHA256SUMS`, and verifies the Linux cosign bundles when
   `cosign` is installed; `pins.toml` records whether it was. Both write the
   result to `pins.toml` at the repository root, and its compiled form to
   `crates/agents/src/pins.rs`, because the subset has no `include_str!`.
2. `conductord` compiles the pins in. On start it makes sure the pinned CLI
   is at `<data>/cli/<provider>/<version>/`: `claude` there, and Codex's
   package unpacked whole, with `bin/codex`. It downloads to a temporary
   file, checks the SHA-256 of that file as written to disk, and renames it
   into place (the package is unpacked with the system `tar` beside it
   first, then the directory is renamed), so a crash leaves either nothing
   or a verified install. It holds a lock on `<data>/cli/<provider>/lock`
   throughout, so a second process on the same data dir, such as
   `conductord login` during a first start, waits and then finds the CLI
   installed. A wrong hash, or a download that fails, makes the provider
   `Unavailable` with the reason, as a missing CLI does today. A connection
   gets 15 s and a stalled download 30 s, so a dead network doesn't hold
   up the host's start for long. On Linux it takes Claude's glibc
   build; Codex publishes only musl builds for Linux. A musl host such as
   Alpine is not handled yet.
3. In the same step, under the same lock, it deletes everything else in
   `<data>/cli/<provider>/`: older versions, and any download or unpack a
   crash left. This runs at start, before any session, so no session is
   using an older version.

This keeps OpenPGP and Sigstore out of the daemon. The trust is in the
developer's `tools/pin` run, which upstream's signatures back, and in the
Conductor build that carries the hashes.

`--claude-path` and `--codex-path` stay, for the fake CLIs in tests and for
development. Without them the daemon runs only the pinned copies.

#### The pin, the fixtures and the live checks

The pin and the protocol code move together. Bumping a pin is one commit
that:

1. runs `tools/pin` for the new version;
2. runs `s1` or `s2` against the pinned binary, which installs it into
   `target/conductor-live` with the same installer and runs it with the
   user's own homes and logins, and re-records `fixtures/claude` or
   `fixtures/codex`;
3. widens `SUPPORTED_VERSIONS` and `check_version`
   (`crates/agents/src/claude/protocol.rs`) to include it.

A test reads `pins.toml` and the version recorded in each fixture and fails
when a pinned version has no fixture recorded at that version, or falls
outside the supported range. The rule lives in that test rather than in
this paragraph.

#### Bundling is out

Conductor never ships either binary inside its own release. Claude Code's
`LICENSE.md` reads "© Anthropic PBC. All rights reserved. Use is subject to
the Legal Agreements" ([license], verified), which grants no right to
redistribute (inferred, because neither the license nor the terms has a
redistribution clause to rely on). The
legal page also says that running Claude Code in a product requires that
"Claude Code must be installed and run as published by Anthropic" and
that each end user authenticates with their own credentials ([legal],
verified). Conductor follows both. The binary is downloaded unmodified from
Anthropic, and each owner signs in with their own account. Codex is
Apache-2.0 and could be bundled, but one mechanism for both CLIs is simpler,
so Codex is downloaded the same way.

#### Each host gets private CLI homes

| | Directory | Set by |
|---|---|---|
| Claude config, credentials, transcripts | `<data>/home/claude` | `CLAUDE_CONFIG_DIR` |
| Codex config, `auth.json`, rollouts | `<data>/home/codex` | `CODEX_HOME` |

`CLAUDE_CONFIG_DIR` moves "all settings, session history, and plugins",
`.claude.json` included, and on macOS keys the Keychain entry to the
directory ([env-vars], [authentication], verified). `CODEX_HOME` holds
`config.toml`, `auth.json` and `sessions/` (`codex-rs` in [codex-src],
verified). So a host's own `~/.claude` and `~/.codex` stay exactly as the
user left them.

Two consequences for how sessions start (§5.2):

- Claude sessions run with `--setting-sources user,project,local` instead of
  `project,local`. The user level is now Conductor's own
  `<data>/home/claude/settings.json`, which sets `cleanupPeriodDays` high
  enough that Claude Code never sweeps a transcript Conductor still needs
  (the default is 30 days, [settings-reference], verified). Conductor deletes
  a transcript when it deletes the session.
- Each Claude process gets `CLAUDE_CODE_PROJECT_DIR_NAME=<session ulid>`, so
  its transcript lives at `<data>/home/claude/projects/<ulid>/` whatever the
  working directory is ([sessions], verified; it needs 2.1.234 or later).
  §6.8 relies on this. It lands with the ULID session ids (§6.7), in the
  durability phase: today's `SessionId` is a host counter that restarts at
  1, so two sessions would share a transcript directory.
  Until then a transcript is in `projects/<the working directory, / as
  ->/<uuid>.jsonl`, under a UUID the host picks with `--session-id` when
  the CLI first starts. `Continue` resumes by that UUID, which `--resume`
  finds in any project directory, so two sessions in one directory never
  share a conversation (observed). What waits for the ULIDs: the host
  keeps the UUID only in memory, so after a host restart nothing links a
  session to its transcript; Conductor can't delete a session's
  transcript with it, since the directory is shared by every session in
  that working directory; and a move (§6.8) can't copy one directory per
  session.

`conductord` writes `<data>/home/claude/settings.json` (`cleanupPeriodDays`
36500) and `<data>/home/codex/config.toml` (`check_for_update_on_startup =
false`) on first run only, because the CLIs edit their own config too. Every
`claude` it starts gets `DISABLE_UPDATES=1`, `s1` included, so a live check
can't update the user's own install either. In a private home it also loses
these variables from the host's environment, each of which takes it off the
home's own sign-in: `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN` and
`CLAUDE_CODE_OAUTH_TOKEN` take precedence over it,
`CLAUDE_CODE_USE_BEDROCK`, `CLAUDE_CODE_USE_VERTEX` and
`CLAUDE_CODE_USE_FOUNDRY` switch to a cloud account, `ANTHROPIC_BASE_URL`
sends requests elsewhere, and `CLAUDE_CONFIG_DIR` names another home.
Codex loses an inherited `CODEX_HOME`. Observed: a shell inside Claude Code
carries `CLAUDE_CODE_OAUTH_TOKEN`, and `claude auth status` then reports a
login in an empty home; with `CLAUDE_CODE_USE_BEDROCK=1` it reports
`"apiProvider": "bedrock"`. With the scrub, each one leaves `"loggedIn":
false`. Even `--version` runs with the
private home set, because `codex --version` writes helper binaries into
`CODEX_HOME/tmp` (observed).

Until the app can sign in (§6.3), `conductord login claude|codex` runs the
sign-in on the host's terminal against the private home: `claude auth login
--claudeai`, §6.3's per-host mode, which stores its own credential, and
`codex login --device-auth`. A private home signed in this way counts as
configured (`Auth::SignedIn`, §6.3). The host checks it once, when it
starts, with `claude auth status` and `codex login status` (15 s at most
each), so a sign-in made while the host runs, or one that expires, shows
only after a restart.

Optional host setup on Linux and WSL2: `sudo apt install bubblewrap
socat`. With them Claude runs its commands in its own sandbox and the
host turns on its environment scrub, so Claude's commands can't read a
gateway token (§6.3). Codex sandboxes its commands itself and needs
nothing. Conductor packages neither.

The credential store of §6.3 exists
now, with gateway credentials as its first kind; the shared `setup-token`
token becomes another kind in it.

### 6.3 Credentials, entered in the app

#### What each provider accepts

| | Claude Code | Codex |
|---|---|---|
| API key | `ANTHROPIC_API_KEY`, which takes precedence over a subscription login ([authentication], verified) | `account/login/start { type: "apiKey" }` on the app-server, or `codex login --with-api-key` from stdin ([codex-auth], `v2/account.rs` in [codex-src], verified) |
| Subscription | `claude auth login --claudeai`. On a machine without a browser it prints a URL; the browser that opens it "shows a code instead of returning to `localhost`", and `claude auth login` "reads the pasted code from standard input" ([troubleshoot], verified). | Device code: `account/login/start { type: "chatgptDeviceCode" }` returns `{ loginId, verificationUrl, userCode }` and `account/login/completed` reports the result ([codex-src], verified). It is in beta and the owner must first "Enable device code login in your ChatGPT security settings" ([codex-auth], verified). |
| Long-lived token | `claude setup-token` prints a one-year OAuth token for `CLAUDE_CODE_OAUTH_TOKEN` and saves nothing ([authentication], verified) | None for ChatGPT. `chatgptAuthTokens`, where the host app supplies the tokens, is marked "[UNSTABLE] FOR OPENAI INTERNAL USE ONLY – DO NOT USE" ([codex-src], verified). |

The browser flow, `account/login/start { type: "chatgpt" }`, returns an
`authUrl` whose callback is the app-server's listener on the host's
`localhost:1455` ([codex-app-server], [codex-src], verified). A phone can't
reach that, so it is not offered.

#### Built first: gateway credentials

The owner reaches both models through a gateway today, so that kind is the
first one built. The others in the table become variants of the same
types.

- **Claude:** `ClaudeCredential::Gateway { base_url, token }`. A Claude
  session gets `ANTHROPIC_BASE_URL` and `ANTHROPIC_AUTH_TOKEN` in its
  environment when it starts. The private home already scrubs both from
  the host's environment (§6.2), so only the stored values reach it.
  Where Claude's sandbox can run, it also gets
  `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1`, so the commands, hooks and MCP
  servers Claude starts don't inherit the token ([env-vars], verified; the
  scrub matches a credential by name or by the shape of its value, from
  2.1.251). On Linux and WSL2 the scrub runs every command in that
  sandbox, which needs `bubblewrap` and `socat`, and without them every
  Bash command fails ([sandboxing], verified; observed). So the host sets
  the scrub on macOS always, on Linux and WSL2 when `bwrap` and `socat`
  are on `PATH`, and never on native Windows, where Claude runs commands
  unsandboxed ([sandboxing], verified). Without it, a stored gateway makes
  the host log "Claude's commands can read the gateway token; install
  bubblewrap and socat to sandbox them" at start and when the gateway is
  set. The base URL reaches commands either way; it is an address, not a
  secret. The live canary in `tools/live-host.sh` has the agent count the
  token in a command's environment: 0 with the sandbox, and 1 without it,
  reported as `leak=unprotected` (observed both). A change applies to the
  next session. Clearing a Claude credential
  doesn't stop the sessions already running on it, unlike Codex below; to
  cut off a leaked token, revoke it at the gateway.
- **Codex:** `CodexCredential::Provider { name, base_url, key, model }`.
  Before the app-server starts, the host writes
  `[model_providers.conductor]` (`name`, `base_url`, `env_key =
  "CONDUCTOR_CODEX_KEY"`, `wire_api = "responses"`, the only one Codex
  0.166 knows) and `model_provider` and `model` into the private
  `config.toml`, keeping the rest of the file, and passes the key in
  `CONDUCTOR_CODEX_KEY`. The key is never in the file. Codex 0.166 passes
  variables named like a key to the commands it runs
  (`ignore_default_excludes` defaults to true in
  `config/src/shell_environment_policy.rs` at `rust-v0.166.0`, verified),
  so the config also gets `[shell_environment_policy.filters]
  CONDUCTOR_CODEX_KEY = "exclude"`; the live canary counts it 0 in a
  command's environment (observed). The app-server reloads
  `config.toml` for each `thread/start` (`thread_processor.rs` in
  [codex-src], verified), but the key is in its environment, which is fixed
  when it starts. So a change stops the shared app-server, its sessions
  close with `CloseReason::Stopped { reason: "new Codex credential" }`,
  and the next thread starts a server with the new key.
- **Protocol:** `SetCredential { credential }` and `ClearCredential {
  provider }` answer `Providers`, what the host runs now, and the client's
  status takes it at once. Another client sees the change at its next
  `Welcome`. Like every command they need a client admitted by the ticket's
  token, which the device certificates of §6.1 replace.
- **Write-only:** `Availability::Available { auth }` reports `Gateway {
  base_url }`, `SignedIn` or `Unmanaged` (a `--claude-path` or
  `--codex-path` CLI), and never a token or key. The credentials' `Debug`
  prints only their kind, so no command log line carries a value. A
  provider with no credential, no signed-in private home and no
  development CLI is `Unavailable { reason: "no credential configured" }`.
  A stored credential takes precedence over the home's sign-in. A base URL
  with a user, a password, a query or a fragment is refused, since it is
  shown to every client and written into Codex's config.

#### Claude: one subscription token for the fleet, by owner decision

Anthropic's legal page says, verbatim: "developers may not collect, store,
or intermediate Claude.ai credentials or session tokens — sign-in to a
Claude account must complete through Anthropic's own flow." It also says
"Anthropic reserves the right to take measures to enforce these
restrictions and may do so without prior notice" ([legal], verified). The
same page allows provisioning one's own API keys in "a development
environment, secrets manager, or machine image".

**Owner decision F2 (2026-11-12).** Conductor stores one `claude setup-token`
token, encrypted, and copies it to every host through the signed fleet
config, the same way it shares API keys. Storing and copying the token is
what the clause above describes. The owner has read it and accepts that
grey area for their own account and their own tool. The risk is action
against the Claude account, possibly without notice. The binary stays
unmodified, and the sign-in itself still happens on Anthropic's page.

Technically, the token can be shared. `setup-token` mints "a one-year OAuth token",
prints it and saves nothing, and needs a Pro, Max, Ultra, Team or
Enterprise plan ([authentication], verified). It does not refresh: "To
replace an expired token, generate a new one and restart" ([env-vars],
verified). So unlike a `/login` session it has no rotating refresh token,
and many hosts can use it at once without logging each other out
(inferred from those two facts; experiment E2 confirms it). It "can only
make model requests", with no Remote Control or connectors
([authentication], verified), and Conductor needs neither.

Each host passes it as `CLAUDE_CODE_OAUTH_TOKEN` when it spawns `claude`.
`ANTHROPIC_API_KEY` takes precedence over it ([authentication], verified),
so the fleet setting `claude_auth: ApiKey | Subscription` decides which of
the two a host sets, and a host never sets both. The fleet config records
when the token was issued, and the app warns 30 days before the year is
up; renewing is the same sign-in again.

**The fallback stays.** If the owner turns sharing off, or Anthropic acts
on it, each host signs in with Claude Code's own `claude auth login`, which
stores its credential in the host's private `CLAUDE_CONFIG_DIR`, and
Conductor stores nothing for Claude subscriptions. §6.6 describes that mode.

#### Codex: API key shared, ChatGPT login per host

Conductor stores the OpenAI API key in the fleet config. A ChatGPT login
can't be shared, for a technical reason (§6.6). The Codex app-server on
each host makes its own login and keeps it in that host's `CODEX_HOME`.

#### Sign in from the phone

The host runs the CLI's sign-in and the phone does the browser part.

1. In the app, the owner picks *Sign in to Claude* or *Sign in to Codex* on
   a host. The app sends `StartLogin { provider }`.
2. **Claude, shared token.** The host runs `claude setup-token` with its
   private `CLAUDE_CONFIG_DIR`, on pipes, and reads the URL it prints. The
   app shows *Open Anthropic sign-in*. The owner signs in on Anthropic's
   page, which shows a code, pastes the code into the app, and the app
   sends `SubmitLoginCode { login, code }`. The host writes the code to the
   CLI's stdin, reads the token the CLI prints, and sends it to the root
   host, which puts it in the fleet config (§6.5). Relaying the code
   through the app is an owner decision (F5, §6.10). That the CLI prints
   the URL, reads the code and prints the token when its stdin and stdout
   are pipes is inferred, and experiment E1 checks it; if it needs a terminal, the host
   runs it under a pseudo-terminal.
3. **Claude, per-host fallback.** The same flow with `claude auth login
   --claudeai`, which "reads the pasted code from standard input"
   ([troubleshoot], verified) and stores the result itself. Nothing goes to
   the fleet config.
4. **Codex.** The host calls `account/login/start { type:
   "chatgptDeviceCode" }` on its app-server. The app shows the
   `verificationUrl` and the `userCode` with a copy button. The owner opens
   the URL and enters the code, and `account/login/completed` ends the
   flow. Codex writes `auth.json` in the host's `CODEX_HOME`.
5. **API keys.** The app has one field per provider. `SetCredential {
   credential }` goes to the root host, which puts it in the fleet config,
   so every host gets it.

The login URL, the user code and the pasted code are live-only messages.
They are not events in the log, are never written to the store, and appear
in no trace output. The Claude token goes from the host that minted it to
the root host on `conductor-fleet` and never reaches the browser.

`Welcome.providers` says whether the host can run each provider and how it
signs in: `Availability::Available { auth }`, built with `Gateway`,
`SignedIn` and `Unmanaged` and growing `ApiKey` and `Subscription`, or
`Unavailable { reason }`. The sidebar shows a host that needs a sign-in,
with the button.

#### The browser holds a secret only in flight

The key field is `type="password"` with `autocomplete="new-password"`,
since password managers ignore `off` there. The app sends
the value on the encrypted iroh link, clears the field, and never writes it
to IndexedDB, local storage, the URL or a log. Under the field the app says
so: "Sent to desk-wsl over the encrypted link. This app doesn't keep it;
your browser may offer to save it." A
pasted Claude code gets the same treatment. The app never reads a secret
back; the host reports only which kind is set, with a gateway's base URL.
The credentials screen built first does this for a gateway token and a
Codex provider key.

#### Storage on the host

Conductor stores the fleet's credentials (the API keys and the Claude
token) and its own keys (the host's iroh key and, on the root host, the
fleet root key). They are encrypted in the store with
XChaCha20-Poly1305 under a 32-byte **host key**.

- On macOS the host key lives in the login Keychain, on Windows it is
  sealed with DPAPI for the user, and on Linux it goes in the Secret Service
  when a session bus offers one. Otherwise it is `<data>/store.key`, with
  mode 0600 on Unix. On Windows the file gets the data dir's ACL, which is
  the user's own under the default profile directory and may be wider
  under a `--data-dir` elsewhere. (`<data>/host.key` holds the iroh key
  until the store holds it.) Built so far: the file variant on every
  platform; the platform keystores are follow-up work. A new key is
  written whole and then linked to its name, so two first starts agree on
  one key.
- The credentials are one value, `proto::Credentials`, one optional
  credential per provider, in `<data>/credentials`: a format byte, a fresh
  24-byte nonce and the postcard sealed with the format and the file's
  purpose as associated data. A save writes and syncs a new file and
  renames it over the old one, then syncs the directory (Unix). It is the
  `credentials` field of the fleet config (§6.5), so sharing it is a copy.
- A store that doesn't open (corrupt, or sealed under a lost key) leaves
  the host running with both providers `Unavailable` for that reason,
  since it may hold a credential that would come before a sign-in.
  `SetCredential` is refused then, so nothing overwrites it. A
  `ClearCredential` for either provider moves it to
  `<data>/credentials.unreadable` and starts an empty store, which is the
  recovery from the phone. Only a `store.key` that can't be read stops the
  host.
- A Claude gateway token is kept from Claude's commands only where
  Claude's sandbox runs, which on Linux and WSL2 is the optional
  `sudo apt install bubblewrap socat` of §6.2. The Codex key is kept from
  Codex's commands everywhere.
- Most hosts here are WSL or headless Linux, where there is no Secret
  Service. There the encryption protects copies of the store (a backup, a
  store attached to a bug report) and not the running host, because the key
  sits beside it. File permissions are the real protection, as they are for
  the CLIs' own `~/.claude/.credentials.json` and `~/.codex/auth.json`
  ([authentication], [codex-auth], verified). The design says this
  plainly instead of promising more.
- Credentials reach the CLIs only at spawn. Claude gets `ANTHROPIC_API_KEY`
  or `CLAUDE_CODE_OAUTH_TOKEN` in its environment, so Conductor never writes
  either into the CLI's home. The Codex app-server gets `account/login/start { type:
  "apiKey" }` after `initialize`, with `cli_auth_credentials_store =
  "ephemeral"` in its config when the host uses a key, so Codex keeps it in
  memory only (the store options in `config/src/types.rs`, [codex-src],
  verified; that `ephemeral` writes nothing to disk is inferred from its
  name). A host signed in with ChatGPT uses the `file` store, because Codex
  must keep its refreshed tokens across restarts.

### 6.4 Join a host to the fleet

A host joins with a one-time invite, from either side.

- **From the app.** *Add host* asks the root host for an invite and shows
  `conductord join <ticket>`. The ticket holds the root host's node id, its
  home relay and a random 32-byte invite secret that expires after one hour
  and works once. Running the command on the new machine dials the root host
  on `conductor-fleet`, proves the secret, and receives a `Host` certificate
  for its node id and the current fleet config.
- **From the host.** A `conductord` started with no fleet prints its own
  ticket, as it does today. Scanning it in a paired app sends it to the root
  host, which dials the new host and admits it the same way.

Joining is idempotent. An invite already used by the same node id returns the
same certificate, so a join that died after the reply can run again. The new
host then installs the pinned CLIs (§6.2) and can run sessions at once with
the fleet's API keys and Claude token. Codex with a ChatGPT login needs one
sign-in on the new host (§6.6), and the app asks for it as soon as the host
appears.

Leaving is revocation. The root host adds the node id to `revoked` in a new
fleet config version, and every host refuses it from then on. A revoked host
has seen the fleet's credentials, so the app tells the owner to rotate the
API keys, issue a new Claude token, and sign that host out of ChatGPT on
OpenAI's account page. Whether an old `setup-token` token can be revoked
before its year is up is not documented (not verified).

### 6.5 The fleet config: one signed document any host can serve

What the fleet shares is one document:

```
FleetConfig {
    fleet: FleetId,
    version: u64,                 // +1 on every change
    roster: Vec<HostEntry>,       // node id, name, home relay, added_at
    revoked: Vec<NodeId>,         // devices and hosts
    pins: Pins,                   // §6.2
    settings: FleetSettings,      // default models, effort, permission modes
    credentials: Credentials,     // API keys, Claude token; sent only to hosts
}
SignedFleetConfig { config: FleetConfig, signature: [u8; 64] }  // by the root key
```

- **One writer.** Only the root host signs, so versions never fork and there
  is nothing to merge. A change from the app (a new credential, a pin bump, a
  new host) is a command to the root host, which applies it, bumps the
  version, signs and stores it.
- **Any host serves it.** Every host keeps the newest valid config it has
  seen in its store. When two hosts connect on `conductor-fleet`, each sends
  its version; the one behind asks for the document and checks the signature
  and that the version is higher before it adopts it. The root host also
  pushes each new version to every host it can reach.
- **An offline host catches up** the next time it reaches any other host,
  whether that is the root host or not. A host that comes back after a long
  time adopts the newest version, installs any new pin and drops revoked
  devices before it accepts a client.
- **Only hosts get the credentials.** The whole config goes only to peers with a
  `Host` certificate, on the authenticated, encrypted `conductor-fleet`
  link. Each host stores it encrypted under its host key (§6.3).
- **Clients see the rest.** `Welcome` carries the host's config version and its
  roster, so the app learns new hosts from whichever host it reaches first,
  and the sidebar shows a host that runs an older config.

The config is small (a few kilobytes) and changes rarely, so whole-document
transfer is enough. There is no gossip protocol, CRDT or per-field merge.
The single writer costs nothing extra here, because a signature from one key
is already what makes the roster trustworthy.

### 6.6 Which subscriptions a fleet can share

The owner wants a new host to share credentials. API keys and the Claude
`setup-token` token are shared through the fleet config (§6.3). A Codex
ChatGPT login can't be, and Codex falls back to one sign-in per host.

#### Codex: one ChatGPT sign-in per host

This is a technical limit, not a policy choice.

- **Codex rotates its ChatGPT refresh token.** Codex refreshes when the
  access token is within 5 minutes of expiry or the last refresh is more
  than 8 days old, and writes the new tokens back to `auth.json`
  (`TOKEN_REFRESH_INTERVAL`, `persist_tokens` in
  `codex-rs/login/src/auth/manager.rs`, [codex-src], verified). A reused
  refresh token fails with `refresh_token_reused`: "Your access token could
  not be refreshed because your refresh token was already used. Please log
  out and sign in again." (same source, verified). OpenAI's CI guide says
  "Do not share the same file across concurrent jobs or multiple machines",
  because "another machine or concurrent job rotated the token first"
  ([codex-ci], verified). Two hosts on one `auth.json` would log each other
  out.
- **A single refresher isn't available.** It would need Codex to accept
  tokens from Conductor and ask Conductor to refresh them. That is
  `chatgptAuthTokens` with the `account/chatgptAuthTokens/refresh` server
  request, which is marked for OpenAI's internal use only ([codex-src],
  verified). Building on it would break without notice, and Conductor would
  then hold ChatGPT tokens it has no business holding.

So each host signs in to ChatGPT once, from the phone, by §6.3's
device-code flow. After a join, the new host appears with *Sign in to
Codex* when the fleet uses Codex by ChatGPT login. Inside one host, every
Codex thread shares the one app-server, so Codex's own refresh coordination
applies (`auth_storage_commit_guard` in [codex-src], verified).

Whether a ChatGPT sign-in on a second host invalidates the first host's
login is not documented. Experiment E3 tests it. If it does, Codex by
ChatGPT can run on one host at a time, and the app says so before the
second sign-in.

#### Claude: shared by owner decision, with a per-host fallback

Sharing the Claude token is owner decision F2 (§6.10). Anthropic's legal page
says "developers may not collect, store, or intermediate Claude.ai
credentials or session tokens" ([legal], verified), and the owner accepts
that grey area for their own account and their own tool. The risk is that
Anthropic acts against the account, which the same page says it may do
"without prior notice".

So the per-host mode stays designed, for when sharing is turned off. In it,
each host runs `claude auth login` and Claude Code keeps its own credential.
That login is not shareable either. Claude Code refreshes its `/login`
token and coordinates so "only one process refreshes"; before 2.1.211, two
sessions renewing with the same token "revoked the saved login"
([authentication], [troubleshoot], verified). That points to a rotating
refresh token (inferred), so a copied `.credentials.json` would fail the
way a shared Codex `auth.json` does. Inside one host, every Claude process
shares the host's `CLAUDE_CONFIG_DIR`, so Claude Code's coordination
applies. Experiment E2 tests whether a second host's login invalidates the
first.

### 6.7 Fleet-unique session ids and branch names

`SessionId` is a host counter today (`crates/proto/src/lib.rs`), unique only
within an epoch. A session that moves keeps its id, and the app merges
sessions from every host, so ids must be unique across the fleet and across
restarts.

- **Format.** A `SessionId` is a 128-bit ULID: 48 bits of Unix milliseconds
  and 80 random bits, generated by the host that creates the session. On the
  wire it is 16 bytes; as text it is the 26-character Crockford base32 form,
  such as `01JCDZ6Q0Y3K7V8M2N4P5R6S7T`. No coordination between hosts is
  needed, because two ids collide only if they share the millisecond and all 80
  random bits. Ids sort by creation time, which the sidebar uses.
- **Branch.** `conductor/<ulid>`. A session can carry a title, but the title
  is not in the branch name, so renaming never touches git.
- **Worktree.** `<data>/worktrees/<project>/<ulid>`.
- **Checkpoints.** `refs/conductor/<ulid>/<turn>`, as in §5.3.
- **Short form.** The UI shows the last 6 characters where space is tight,
  and the full id on hover or long-press.

`HostSeq` and `Epoch` stay per host (D4). An event names its session by id,
so the same id can appear in two hosts' logs after a move (§6.8).

The id change lands with durability, before redb tables key on it.

### 6.8 Move a session to another host

A session is three things: its worktree branch with its checkpoint refs, the
CLI's own transcript, and its events in the host's log. A move copies all
three to the target host and makes the target the session's only owner.
D4 still holds: at any moment one host owns a session, and nothing
replicates continuously.

The app sends `MoveSession { session, to: NodeId }` to the host that owns
the session. That host, the source, drives the move over `conductor-fleet`
under a `MoveId`, and every step is safe to repeat.

1. **Settle.** The source refuses while a turn is running or a request is
   open; the app offers *Interrupt and move*. Otherwise it marks the session
   `Moving`, which refuses `SendMessage`. Then it stops the agent. It ends the
   Claude process, or drops the Codex thread from `CodexHub`.
2. **Check the target.** The target must run the same CLI pin, which holds
   in one fleet unless an upgrade is half done, and must be signed in to
   the provider. It must also have the project, meaning a
   clone of the same repository, matched by the root commit and the `origin` URL.
   A target without one clones it from `origin`; a project with no remote is
   sent whole in the bundle.
3. **Send the branch.** The source snapshots the worktree, uncommitted
   changes included, as one more checkpoint (§5.3), then streams
   `git bundle create - conductor/<ulid> refs/conductor/<ulid>/*` excluding
   the commits the target says it already has. The target runs `git bundle
   verify` and `git fetch` from it. A bundle over the iroh stream is the
   default rather than a push, because a push to the user's remote has side
   effects the user didn't ask for (CI runs, a visible branch) and fails
   when the remote is unreachable.
4. **Send the transcript.** The source sends the CLI's session files and the
   target writes them where its own CLI looks, unchanged (§6.8.1).
5. **Send the events.** The source sends the session's durable events. The
   target appends them to its log with its own seqs, after a
   `SessionArrived { from, move_id, source_epoch, source_last_seq }` event,
   and creates the worktree at its own `<data>/worktrees/<project>/<ulid>`
   from the last checkpoint.
6. **Commit.** The target registers the session and acknowledges. The
   source then appends `SessionMoved { to, move_id }`, removes its worktree
   and its copy of the transcript, and keeps the branch until the next move
   or deletion. The next `SendMessage` on the target resumes the agent with
   the provider's resume (`--resume <session_id>`, `thread/resume`).

The source owns the session until the target's acknowledgement arrives. If
the source crashes before that, it restarts with the session `Moving`, asks
the target about the `MoveId` and either finishes step 6 or returns the
session to idle. If the target crashed, the source retries from step 2, and
the target's receipt for the `MoveId` makes each step converge.

#### 6.8.1 Resume in another directory

The worktree path differs between hosts, and between operating systems it
can't be made the same. Each CLI handles that differently.

- **Claude.** With `CLAUDE_CODE_PROJECT_DIR_NAME=<ulid>` (§6.2), the
  session's transcript and auto memory are the one directory
  `<data>/home/claude/projects/<ulid>/` ([sessions], verified). The move
  copies that directory as it is. `claude --resume <session-id>` works "from
  any directory" and searches every project for the id, refusing only when
  two copies make it ambiguous ([sessions], verified), and there is one
  copy, so the target resumes it from the new worktree. Each record in the
  `.jsonl` keeps the old absolute `cwd` (verified, observed), and Anthropic
  says the format "is internal to Claude Code and changes between versions"
  ([sessions], verified), so Conductor never rewrites the file. That a
  resume on another machine works is inferred: no document covers it, and
  a request for shared-filesystem sessions was closed as not planned
  ([cc-34024], verified). Experiment E4 tests it.
- **Codex.** Rollouts are
  `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<timestamp>-<uuid>.jsonl`, with an
  index in `session_index.jsonl` and a SQLite state database
  (`codex-rs/rollout`, `codex-rs/state` in [codex-src], verified). Resume by
  id asks SQLite first and falls back to matching file names
  (`find_thread_path_by_id_str`, verified). The move copies the rollout to
  the same relative path in the target's `CODEX_HOME`, and the target calls
  `thread/resume { threadId, cwd: <new worktree> }`. `ThreadResumeParams`
  has the `cwd` override and an unstable `path` field ([codex-src],
  verified); Conductor uses the id and `cwd`, not `path`. That the file-name
  fallback finds the copied rollout and that `cwd` takes effect is
  inferred. Experiment E5 tests both. The rollout records the ChatGPT
  account that created it (`creator_account_id`, verified); a target
  signed in to a different account may hide it from `thread/list`
  (inferred), which Conductor doesn't use.
- **Both.** The model's context still names the old paths. The first
  message after a move gets one line from the host before the user's text:
  "This session moved to another machine. The working directory is now
  `<path>`." Without it, the model may use a path from earlier in the
  conversation that no longer exists (inferred).

#### 6.8.2 What a client sees

- **During.** The session stays in place in the sidebar with *Moving to
  laptop-mac* and the current step. The composer and the interrupt button
  are disabled, and the transcript stays readable.
- **After.** The app is already connected to every host in the roster.
  `SessionMoved` on the source and `SessionArrived` on the target carry the
  same `MoveId`, so `HostView` folds them into one session. It shows the
  target's copy under the target host, keeps the selection on it, and hides
  the source's copy. The transcript reads as one, because the target holds
  the whole history; a divider says where the move happened.
- **If the target is offline** to this client, the app shows the source's
  copy read-only, marked *Moved to laptop-mac (offline)*.
- **If the move fails**, the session returns to idle on the source with the
  reason, and nothing changed on the target that a retry won't reuse.

Diffs and revert for turns before the move still work on the target,
because the checkpoint refs moved with the branch and the events moved with
the log.

### 6.9 Experiments

None of these has been run. Each touches a real Claude or ChatGPT account.

**Owner consent (2026-11-12).** Each experiment may run in the phase that
needs it, in scratch `CLAUDE_CONFIG_DIR` and `CODEX_HOME` directories only,
so the owner's own `~/.claude` and `~/.codex` are never read or written.
The coordinator tells the owner before each run. A throwaway account is
better where one exists.

| | Question | Experiment | Risk to the account |
|---|---|---|---|
| E1 | Does `claude setup-token` print its URL, read the code and print the token when stdin and stdout are pipes? And `claude auth login --claudeai`, for the fallback? | Run each with a scratch `CLAUDE_CONFIG_DIR`, stdin and stdout piped to a script that prints the URL and forwards one line. Complete the sign-in in a browser on another device. Check that `setup-token` printed a token, and run `claude auth status` after `auth login`. | A new token and a new login appear on the account. Sign out of the login afterwards. |
| E2 | Can two hosts use one `setup-token` token at once, and does a second `claude auth login` on another host invalidate the first? | Part 1: set one token as `CLAUDE_CODE_OAUTH_TOKEN` on hosts A and B (scratch dirs, separate machines) and run turns on both at the same time, daily for 4 days. Part 2: sign in A and B with `claude auth login` to the same account, run one turn on each daily for 4 days, past a refresh, and record `claude auth status` on both. | Part 2 may log A out once. |
| E3 | Does a second ChatGPT device-code login invalidate the first, and does sharing one `auth.json` break as documented? | Part 1: device-code sign-in on A and B with the same account. Call `account/read { refreshToken: true }` on A, then on B, then a turn on each; repeat on day 9. Part 2: copy A's `auth.json` to a scratch home C, force a refresh on A, then on C, and expect `refresh_token_reused` on C. | Part 2 is meant to log one copy out. It may also invalidate A's login, which the owner then signs in again. |
| E4 | Does a Claude session resume on another machine with a different working directory? | On A, start a session with a scratch `CLAUDE_CONFIG_DIR` and `CLAUDE_CODE_PROJECT_DIR_NAME`, do two turns that read and edit files. Copy the project directory and the git branch to B under a different path, run `claude --resume <id>` from the new worktree with the stream-json flags, and ask what it did before. Check that it answers from history and edits the new path. | Uses the account's quota; an API key works and avoids the subscription. |
| E5 | Does Codex `thread/resume` find a copied rollout by id and honour `cwd`? | The same as E4 with a scratch `CODEX_HOME`: copy the rollout to the same relative path on B, call `thread/resume { threadId, cwd }`, run a turn, and check the command's working directory. | Uses quota; an API key works. |

E1 decides whether the Claude sign-in needs a pseudo-terminal. E2 confirms
the shared token and settles the fallback. E3 decides whether Codex by
ChatGPT can run on several hosts at once. E4 and E5 decide whether moving
sessions is buildable as designed. Run each before the phase that needs it:
E1 before sign-in from the app, E2 and E3 before fleet and pairing, E4 and
E5 before moving sessions.

### 6.10 Decisions

The owner settled these on 2026-11-12.

| # | Date | Decision |
|---|---|---|
| F1 | 2026-11-12 | **The root key lives on one root host**, the first host, with a passphrase-sealed backup file written at `fleet init`. Fleet changes wait while the root host is offline. Putting the root key on every host was rejected, because then one stolen laptop could admit devices. |
| F2 | 2026-11-12 | **One Claude `setup-token` token is shared across the fleet**, encrypted, through the signed fleet config, against the clause quoted in §6.3, as a grey area the owner accepts for their own account and tool. The risk is action against the account. The per-host `claude auth login` mode stays designed as the fallback (§6.6). |
| F3 | 2026-11-12 | **A Codex ChatGPT login is one sign-in per host**, because refresh tokens rotate and a reused one fails with `refresh_token_reused` (§6.6). |
| F4 | 2026-11-12 | **API keys, Anthropic and OpenAI, are shared fleet-wide** through the fleet config. |
| F5 | 2026-11-12 | **The Claude sign-in code is relayed through the app**, in flight only, to the CLI's stdin on the host (§6.3). |
| F6 | 2026-11-12 | **Codex device-code sign-in is on.** The owner enables it in ChatGPT's security settings. Without it, a ChatGPT login on a headless host needs an SSH tunnel to port 1455, which the phone can't open. |
| F7 | 2026-11-12 | **A move sends the branch as a git bundle over iroh**, not a push to `origin`, so it creates no visible branch or CI run on the remote (§6.8). |
| F8 | 2026-11-12 | **A move between hosts on different pins is refused**, and the app offers to update the target first. The transcript formats are internal and change between versions ([sessions], verified). |
| F9 | 2026-11-12 | **Codex analytics are off** (`analytics.enabled = false` in the hosts' private `config.toml`, [codex-src], verified), as a fleet setting the owner can change. |
| F10 | 2026-11-12 | **Moving sessions comes last** in "Next", after push, because it needs durability, worktrees and the fleet first, and E4 and E5 may change it. |
| F11 | 2026-11-12 | **Experiments E1 to E5 may run** in the phase that needs each, in scratch config directories only, with the owner told before each run (§6.9). |

### 6.11 Sources

[setup]: https://code.claude.com/docs/en/setup
[env-vars]: https://code.claude.com/docs/en/env-vars.md
[sandboxing]: https://code.claude.com/docs/en/sandboxing.md
[authentication]: https://code.claude.com/docs/en/authentication
[troubleshoot]: https://code.claude.com/docs/en/troubleshoot-install.md
[sessions]: https://code.claude.com/docs/en/sessions.md
[settings-reference]: https://code.claude.com/docs/en/settings-reference.md
[legal]: https://code.claude.com/docs/en/legal-and-compliance
[license]: https://raw.githubusercontent.com/anthropics/claude-code/main/LICENSE.md
[npm-claude]: https://registry.npmjs.org/@anthropic-ai/claude-code/2.1.316
[cc-34024]: https://github.com/anthropics/claude-code/issues/34024
[codex-src]: https://github.com/openai/codex/blob/c0cc9fc820f62e4dbdde68214dd6d792d6c8a7ee/
[install.md]: https://github.com/openai/codex/blob/c0cc9fc820f62e4dbdde68214dd6d792d6c8a7ee/docs/install.md
[releases]: https://github.com/openai/codex/releases
[npm-codex]: https://registry.npmjs.org/@openai/codex/0.167.2-linux-x64
[codex-auth]: https://learn.chatgpt.com/docs/auth
[codex-ci]: https://learn.chatgpt.com/docs/auth/ci-cd-auth
[codex-app-server]: https://learn.chatgpt.com/docs/app-server

- Claude Code setup, installers, signing, updates: [setup]
- Claude Code environment variables: [env-vars]
- Claude Code authentication, credential storage, `setup-token`: [authentication]
- Headless sign-in with a pasted code: [troubleshoot]
- Transcripts, resume, `CLAUDE_CODE_PROJECT_DIR_NAME`: [sessions]
- `cleanupPeriodDays`: [settings-reference]
- Terms for running Claude Code and for credentials: [legal]
- Claude Code's license: [license]
- Claude Code on npm: [npm-claude]
- Multi-machine sessions request: [cc-34024]
- Codex source at the commit read (`codex-rs/login`, `codex-rs/app-server-protocol`, `codex-rs/rollout`, `codex-rs/state`, `codex-rs/config`, `.github/workflows`): [codex-src]
- Codex install channels: [install.md]
- Codex release assets and digests: [releases]
- Codex on npm: [npm-codex]
- Codex authentication and device code: [codex-auth]
- Codex in CI, refresh and sharing `auth.json`: [codex-ci]
- Codex app-server: [codex-app-server]

## 7. The client (`crates/client`)

- `HostLink` is one per host. One task owns the connection and walks
  `Disconnected → Connecting → Authenticating → CatchingUp → Live`,
  reconnecting with jittered backoff. It keeps the cursor, so every
  reconnect resubscribes after the last applied seq, with no duplicates and
  no gaps.
- `HostView` folds a host's events into what the UI renders: sessions,
  transcripts, tool cards, open requests, what the composer and the
  interrupt button may do. It has no UI types and is unit-tested.
- The client compiles to wasm32 for the app. A native build exists only as
  the harness for the daemon's e2e tests and the headless example.

## 8. The app (`crates/app`)

Dioxus web, routed through the URL fragment so GitHub Pages never serves an
unknown path. Signals wrap `HostView`; the markup renders what the client
decides.

We port T3 Code's designs, not its code (`apps/web/src/components/`):

| Ours | T3 Code reference | v1 |
|---|---|---|
| Sidebar: hosts, projects, sessions with status | `AppSidebarLayout.tsx`, `ConnectionStatusDot.tsx` | yes |
| Transcript, markdown, code | `ChatView.tsx`, `ChatMarkdown.tsx` | yes |
| Tool-call cards | `CommandBlock.tsx` | yes |
| Approval and question panel | `ComposerContextPermissions.tsx` | yes |
| Diff panel per turn | `DiffPanel.tsx` | yes |
| Branch toolbar, command palette | `BranchToolbar*.tsx`, `CommandPalette*.tsx` | later |
| Terminal | `ThreadTerminalDrawer.tsx` | no |

The phone layout follows T3 Code's `docs/internals/mobile-layout.md`: the
sidebar becomes a stack and approvals get one-tap actions.

**Push (planned):** iOS supports Web Push for Home Screen apps. Each host
keeps a VAPID key, stores each device's subscription, and sends a
content-free push ("desk-wsl: approval needed") when a request opens. The
service worker that shows it is the one handwritten JavaScript file. iOS
suspends a backgrounded web app, which drops its connections; on return the
client catches up from its cursors.

## 9. Testing

- **Replay tests** (`crates/agents/tests`): recorded CLI sessions through the
  parsers, asserting exact events.
- **Daemon e2e** (`crates/daemon/tests/e2e.rs`): the real daemon with fake
  CLIs and native `HostLink`s on loopback: approvals, interrupts, reconnects,
  epochs, two Codex sessions on one server.
- **Browser e2e** (`tools/e2e-browser.sh`): the web app in Chromium and
  WebKit against the real daemon and the fake CLIs, with screenshots in
  `docs/p1/screens/`.
- **Manual smoke** (`docs/p1/smoke.md`): real CLIs, the desktop browser and
  the iPhone on mobile data.

## 10. References

| Source | License | Use |
|---|---|---|
| pingdotgg/t3code | MIT | Adapter behaviour, receipts, checkpoints, UI designs |
| anthropics/claude-agent-sdk-python | MIT | Claude's control protocol |
| openai/codex (`codex-rs/app-server`, its JSON Schema) | Apache-2.0 | Codex's protocol |
| getpaseo/paseo | Apache-2.0 | Provider option sets, timeline epochs |
| BloopAI/vibe-kanban (`executors`) | Apache-2.0 | Second reference for the clients |
| hardbeat920/monocode | MIT | SSH host bootstrap, provider lifecycle |
| n0-computer/iroh, DioxusLabs/dioxus | Apache-2.0/MIT | Transport, UI |
