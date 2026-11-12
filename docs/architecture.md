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
  n0's public relays, then upgrade to a direct WebRTC connection when they
  can (§4).

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
| D3b | Upgrade browsers to direct WebRTC over the authenticated iroh stream | WebRTC is the only direct path a browser has. Without it, every client, including a desktop on the host's own LAN, would send all traffic through n0's relays, which are meant for development. The iroh stream carries the signaling, so no signaling server is needed; if WebRTC fails the client stays on iroh. |
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

- **Commands:** `StartSession { provider, directory }`, `SendMessage`,
  `Interrupt`, `Resolve { request, Approve | Deny }`. Each command gets one
  `CommandResult`.
- **Events:** a session is created, becomes ready, closes; a user message; the
  assistant's text, streamed as deltas and then as the finished message; tool
  calls (`Command`, `FileChange`, `Other`) started and finished; approval
  requests opened and resolved (first decision wins, and a late one is told
  who decided); a turn completes, is interrupted or fails.
- **Durable and live:** every event except the streaming deltas is durable:
  it takes the next `HostSeq` and is replayed on catch-up. Deltas go only to
  subscribers that are live, because the finished message replaces them.

What both providers have belongs in the model. What only one has goes in
typed per-provider options instead of being forced into it: Claude's
permission modes, Codex's sandbox. Raw provider JSON never reaches the UI.

Next in the model (`docs/roadmap.md`): questions to the user (Claude's
`AskUserQuestion`, Codex's `requestUserInput`), reasoning text, "approve for
this session", per-session model and effort, resume, plans.

## 4. Transport

### 4.1 Streams

ALPN is `conductor/<n>`. Every bi-stream starts with one `Open` frame from the
client:

| Stream | Purpose |
|---|---|
| `Open::Hello` (first) | Authenticates and returns `Welcome { host, epoch, head_seq, providers }`. |
| `Open::Command` | One `Command` → one `CommandResult`, then the stream closes. |
| `Open::Subscribe { epoch, after_seq }` | Replays the log after the cursor, then streams live. A cursor from another epoch is refused. |
| `Open::Rtc { sdp }` | Negotiates the WebRTC upgrade (§4.3) and stays open while it lives. |

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

### 4.3 WebRTC upgrade

1. Once the browser's link is live, it creates an `RTCPeerConnection` with
   one data channel and opens `Open::Rtc { sdp }`. The host answers from a
   str0m `Rtc` with `Signal::Answer`, and both sides trickle
   `Signal::Candidate`s on the same stream.
2. **Binding:** each side's DTLS fingerprint is in its SDP, and both SDPs
   travel only on the authenticated iroh stream, which the host checks.
3. **STUN:** both sides ask public STUN servers for their public address
   (`proto::DEFAULT_STUN_SERVERS`, `--stun` on the host). str0m does no STUN,
   so the host encodes the binding exchange itself. There is no TURN.
4. **Host sockets:** one UDP socket per up, non-loopback IPv4 interface, on an
   ephemeral port or the first free port of `--rtc-ports`.
5. **Streams:** every stream but `Hello` becomes an ordered, reliable data
   channel with the same frames, split into messages of at most 16 KiB
   (`proto::CHUNK_LEN`) and reassembled with `proto::FrameBuffer`.
6. **Switching:** when the channel opens, the client subscribes on WebRTC
   from its cursor and drops the relayed subscription; the host's replay
   covers the overlap and the client skips seqs it has. If WebRTC drops, the
   client subscribes on iroh again at once and retries the upgrade later
   (15 s, 30 s, 60 s, 120 s, then every 300 s).

#### Running the host in WSL

With `networkingMode=mirrored`, WSL shares Windows' addresses, but the
Hyper-V firewall blocks inbound traffic to WSL by default, so a phone's ICE
checks never arrive. Start the host with a fixed range and open it for WSL on
private networks only:

```sh
conductord host --rtc-ports 50000-50099
```

```powershell
New-NetFirewallHyperVRule -Name ConductorWebRTC -DisplayName "Conductor WebRTC" -Direction Inbound -VMCreatorId '{40E0AC32-46A5-438A-A0B2-2B479E8F2E90}' -Protocol UDP -LocalPorts 50000-50099 -Profiles Private -Action Allow
```

`Remove-NetFirewallHyperVRule -Name ConductorWebRTC` removes it. The browser
offers its LAN address only as an mDNS `.local` name, which the host skips;
the host learns the phone's address as a peer-reflexive candidate once its
checks arrive.

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
  open request is withdrawn when the turn ends.
- **CodexHub** owns the one `codex app-server` process per host and routes
  its threads to their sessions.
- **Serve** accepts connections, checks `Hello`, dispatches commands and
  feeds subscriptions.

### 5.2 Agents (`crates/agents`)

**Claude Code:** one long-lived process per session, started as

```
claude --input-format stream-json --output-format stream-json --verbose \
       --permission-prompt-tool stdio --include-partial-messages \
       --permission-mode <mode> [--model M] [--resume <session_id>] \
       --setting-sources project,local
```

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
  and `model/list`, and handle `turn/started`, `turn/completed`,
  `item/started`, `item/completed` and `item/agentMessage/delta`.
- Approvals arrive as server requests (`item/commandExecution/requestApproval`,
  `item/fileChange/requestApproval`, `item/permissions/requestApproval`,
  `item/tool/requestUserInput`) and `serverRequest/resolved` withdraws one.
  Decisions are `accept`, `acceptForSession`, `decline`, `cancel`; keep
  Codex's own option values, since a label is not always a valid reply.
- Sessions use Paseo's preset: `on-request` approvals with a
  `workspace-write` sandbox.
- Traps (from T3 Code's provider notes): Codex's asynchronous questions are
  answered with a new user message, not an RPC response; `/goal` can start
  follow-up turns on its own.

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

## 6. Planned: trust and pairing

Today a ticket's random token stands in for a device certificate.

- A **fleet root key** signs `DeviceCert { fleet, device, name, role }` and a
  versioned `Roster { hosts, revoked_devices }`. Hosts store only the root's
  public key and check the certificate in `Hello` against it, the
  authenticated node id and the revocation list.
- The root key lives on a host and is used through `conductord pair`, which
  signs a device's key from a QR code. A browser keeps its device key as a
  non-extractable WebCrypto key in IndexedDB, and iOS gives a Home Screen app
  its own storage, so the phone pairs from the installed app.
- Whoever controls the GitHub Pages deploy controls the code the browser
  runs: protect the deploy branch, use Subresource Integrity, and keep 2FA on.

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
| BloopAI/vibe-kanban (`executors`, `relay-webrtc`) | Apache-2.0 | Second reference for the clients; WebRTC signaling and chunking |
| hardbeat920/monocode | MIT | SSH host bootstrap, provider lifecycle |
| n0-computer/iroh, algesten/str0m, DioxusLabs/dioxus | Apache-2.0/MIT | Transport, host WebRTC, UI |
