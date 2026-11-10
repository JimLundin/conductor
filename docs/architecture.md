# Conductor: architecture sketch (draft for review)

Status: draft, revised 2026-11-10 after these decisions: iPhone with no Apple Developer
account; hosts on Windows, WSL and macOS; no relay of our own; a static web client on a
dedicated domain; Dioxus for the UI. Nothing here is built yet. Items marked **[verify]**
are assumptions that a spike must confirm before we depend on them.

## 1. What we are building

One app that runs and supervises **Claude Code** and **Codex** sessions on **several of
your machines**. It has two clients:

- **Desktop:** a native Dioxus app, Windows first and macOS later.
- **iPhone:** the same Dioxus UI compiled to WebAssembly. It is served as a static web app
  from your domain and installed to the Home Screen.

All traffic is end-to-end encrypted. We run **no servers**. n0's free public iroh relays
are used to introduce peers; after that, traffic goes directly between devices (§5.3, §5.4).

### Goals for v1

- Run Claude Code and Codex sessions on any paired host, one git worktree per session.
- Show all hosts in one interface on desktop (Windows, then macOS) and on iPhone (Safari web app).
- Handle tool approvals, questions, interrupts, resume, and per-turn diffs from any device.
- Catch up correctly after a disconnect, such as a phone waking up.
- Use Rust everywhere. The only handwritten JavaScript is a ~10-line service worker that
  Web Push requires (§10.1).

### Non-goals for v1 (candidates for later)

- Terminal emulator. T3 Code has one, built on libghostty; we skip it.
- Agents other than Claude Code and Codex.
- Planner or orchestrator agent, kanban board, routing CI and review results back to the agent.
  These come from Orchestrator.inc and arrive in a later phase.
- Embedded browser preview, cloud workers, multi-user or team use.

### Closest prior art: try it before building

**MonoCode** (`hardbeat920/monocode`, MIT, about 3.9k stars, created 2026-08) is the only
project we found that passes all four must-haves. Its stack and design:

- Tauri 2 with a Rust core (`crates/monocode-core`).
- Host services installed over SSH (`remote/ssh.rs`, `remote_bootstrap.{sh,ps1}`).
- Direct QUIC with mutual TLS to the phone, using quinn and rustls plus UPnP
  (`crates/mobile-link`, `monocode-core/src/mobile/`).

Where it differs from this design:

- No hole punching and no relay, so a phone behind carrier NAT can't reach a host
  without a VPN. iroh (and WebRTC for the browser) fixes that.
- The phone pairs with the desktop, not with each host.
- There is a single maintainer.

Running it for an hour is a cheap way to check our assumptions. Its Rust files are also
the closest code to borrow (§14).

## 2. Decisions

| # | Decision | Why | Alternatives rejected |
|---|---|---|---|
| D1 | Rust for the daemon, the client core, and the UI | One language, one set of types shared end to end. | A Node daemon. The survey recommended it because Anthropic's SDK is only in TypeScript and Python. We confirmed the SDK is a thin wrapper over a stdio protocol that we can port (§7.1). |
| D2 | UI in **Dioxus 0.7**, with one codebase for two targets: **desktop** (native Rust, system WebView) and **web** (WASM, for the iPhone) | It is the only Rust framework that renders real HTML on the web (needed for Safari: keyboard, scrolling, selection) and also ships its own desktop target. On desktop the UI code runs as native Rust, so iroh runs natively and can hole-punch, with no IPC layer. | Leptos + Tauri 2: better web performance, but two moving parts on desktop. egui, Iced, Slint and Makepad draw to a canvas on the web, which handles keyboard, scrolling and selection poorly in mobile Safari. GPUI has no web target. Known risk: Dioxus is pre-1.0, and 0.8 is in alpha, so plan for one migration. |
| D3 | Transport is **iroh 1.x**, using **n0's public relays** to introduce peers; the relay URL is a setting | Connections are E2EE and dialed by public key. Native clients hole-punch, so the relay carries only setup traffic and the occasional fallback. We run no relay. The relay URL is configurable, so if n0 restricts its free relays (documented as for development and testing) we can switch to a paid or self-hosted one with no code changes. | Our own relay (you don't want to run one). Tailscale/Headscale (takes the phone's VPN slot, vendor coordination). |
| D3b | The browser client uses **iroh over the relay** first, then **upgrades to a direct WebRTC connection** | Browsers can't send raw UDP, so browser iroh can never hole-punch (iroh docs: "All connections from browsers … need to flow via a relay"). WebRTC is the browser's only peer-to-peer path. The relayed iroh connection is already authenticated and encrypted, so it carries the WebRTC setup and no signaling server is needed. If WebRTC fails, the client stays on the relay. | Signaling through the web domain (it is static, with no backend). Staying relayed forever (makes n0's development-only relays a hard dependency for all phone traffic). |
| D3c | The phone client is a **static web app** (installable to the Home Screen) on your domain | No Apple Developer account, so there is no native iPhone app. The domain only serves files. | A native iOS app (needs an account). |
| D4 | **Each host is the only source of truth for its own sessions** | No replication between hosts. Clients connect to each host and merge the results. Catching up is "events after seq N". | Syncing session state between hosts or a central hub. |
| D5 | Trust comes from a **fleet root key** that signs device certificates | Hosts store only the root public key, so pairing a phone once gives it access to every host. Hosts never sync allowlists with each other. | Allowlists replicated between daemons. A compromised host could enrol keys on every other host. |
| D6 | Talk to each agent in its **native protocol**: Claude stream-json control protocol, Codex app-server JSON-RPC. **Use the existing `claude-codes` and `codex-codes` crates** (Apache-2.0, `meawoppl/rust-code-agent-sdks`) for both, and send fixes upstream rather than writing our own clients | These carry the full feature set: approvals, interrupts, resume, fork. T3 Code and Orchestrator.inc do the same for Codex. | ACP. It is the lowest common denominator, built for one client per agent, and Claude would still need a TypeScript adapter. |
| D7 | Shell out to the `git` CLI, not `gix`/`git2` | Worktrees, hooks and the user's git config all behave exactly as on the command line. T3 Code and Orchestrator.inc do the same. | gix has incomplete worktree support. git2 adds a C dependency. |
| D8 | Wire format is length-prefixed JSON over **bidirectional streams**: QUIC streams on iroh, data channels on WebRTC | Easy to debug, tolerant of schema changes, and agent payloads are already JSON. Both transports implement one small `Transport` trait (§5.1), so the protocol doesn't care which carries it. | postcard or bincode. Possible later; the bandwidth isn't a problem. |
| D9 | Hosts run natively on **Windows, WSL and macOS** | That is where your agents live. A Windows machine with both native and WSL agents runs two daemons and appears as two hosts. | — |

## 3. Topology and process model

```
 STATIC WEB DOMAIN (files only, no backend)        n0 PUBLIC IROH RELAYS (not ours)
 ┌─────────────────────────────┐                   ┌─────────────────────────────┐
 │ index.html, app.wasm,       │                   │ introduce peers; carry      │
 │ manifest, sw.js (~10 lines) │                   │ encrypted traffic only when │
 └──────────────┬──────────────┘                   │ a direct path fails         │
                │ download once, cached            └──────▲───────────▲──────────┘
                ▼                                         │ setup     │ setup
 ┌──────────────────────────────┐        ┌────────────────┴──┐  ┌─────┴──────────────┐
 │ iPhone: Safari Home Screen   │        │ Desktop app       │  │ (any host)         │
 │ Dioxus web (WASM)            │        │ Dioxus desktop    │  │                    │
 │ client core (WASM)           │        │ client core       │  │                    │
 │  ① iroh-wasm over relay ─────┼─setup─►│ (native Rust)     │  │                    │
 │  ② WebRTC data channel ══════╪═══╗    │ iroh, hole-punched│  │                    │
 │ device key in IndexedDB      │   ║    │ holds FLEET ROOT  │  │                    │
 └──────────────────────────────┘   ║    └───────┬───────────┘  │                    │
                                    ║ direct     │ direct QUIC  │                    │
          ┌─────────────────────────╨────────────┴──────────────┴──────┐             │
          ▼                              ▼                              ▼            │
 ┌────────────────────┐       ┌────────────────────┐       ┌────────────────────┐    │
 │ HOST "desk-win"    │       │ HOST "desk-wsl"    │       │ HOST "mac"         │◄───┘
 │ Windows native     │       │ WSL2 (Ubuntu)      │       │ macOS              │
 │ Task Scheduler     │       │ systemd --user     │       │ launchd agent      │
 └────────────────────┘       └────────────────────┘       └────────────────────┘
   each runs conductord:
    ├ iroh endpoint (+ WebRTC answerer for browsers, via str0m)
    ├ session actors ── claude ×N (process per active session)
    │                 └ codex app-server ×1 (process per host, many threads)
    ├ worktrees  <data>/worktrees/<project>/<session>
    ├ conductor.db (SQLite event log, the source of truth for this host)
    └ Web Push sender ──HTTPS──► Apple's web push service ──► iPhone   (content-free payload)
```

- **Desktop ↔ host:** native iroh. The relay helps set up hole punching, then traffic goes
  direct. On a LAN, local discovery connects with no relay at all.
- **iPhone ↔ host:** step ① connects iroh-wasm through the relay (always relayed in a
  browser), authenticates, and exchanges WebRTC offers and answers. Step ② moves traffic to a
  direct WebRTC data channel. If step ② fails, everything stays on ①, which is slower but
  still end-to-end encrypted.
- **No host talks to another host,** except to receive a newer signed roster (§4).

### 3.1 Bringing up a host

```
$ conductord init             # creates <data>/{host.key, conductor.db, config.toml}
                              # host.key = iroh SecretKey; its public key IS the host's address
$ conductord install-service  # per OS, see below
$ conductord pair             # prints a QR code + link: {host_id, relay_url, one-time token}
```

| OS | Data dir | Started by | Notes |
|---|---|---|---|
| Windows (native) | `%APPDATA%\conductor` | **Task Scheduler**, at logon, as your user. Not a Windows service, because `claude` and `codex` need your user's logins. | **[verify]** Claude Code on native Windows needs Git for Windows (Git Bash). Spawn `claude.exe` and `codex.exe` directly, with no shell in between. |
| WSL2 | `~/.local/share/conductor` | `systemd --user` (`/etc/wsl.conf`: `[boot] systemd=true`) | WSL can shut down when idle; a Windows logon task that runs `wsl.exe` keeps it alive. Try `networkingMode=mirrored` if hole punching fails behind WSL's own NAT. **[verify in S3]** |
| macOS | `~/Library/Application Support/conductor` | launchd user agent | Self-built binaries run without an Apple account. Copied binaries need `xattr -d com.apple.quarantine` once. |

- **First host:** the desktop app scans or pastes the pairing code. The host stores the fleet
  root *public* key in `config.toml`. The client adds the host to the roster, signs the new
  version and pushes it to every host.
- **Adding more hosts:** same steps. The token is single-use and expires after 10 minutes.
- **Adding the iPhone:**
  1. The desktop shows a QR code for `https://<your-domain>/#join=<offer>`. Everything after
     `#` stays in the browser and is never sent to the domain, so the static server never sees
     the offer.
  2. Safari opens the web app. It generates its device key and sends the public key back to
     the desktop over the relay; the offer contains the desktop's node ID.
  3. The desktop signs a device certificate and returns it with the roster.
  4. Add to Home Screen. The phone can now reach every host. Hosts don't need to be told;
     they check the certificate against the root.

## 4. Trust model

```rust
// conductor-trust
pub struct FleetRootKey(ed25519_dalek::SigningKey);   // desktop only (+ offline backup)
pub struct FleetId(ed25519_dalek::VerifyingKey);      // stored on every host & client

pub struct DeviceCert {               // "this iroh NodeId is a member of the fleet"
    pub fleet: FleetId,
    pub device: iroh::PublicKey,      // the client's iroh node id
    pub name: String,                 // "Jim's iPhone"
    pub role: Role,                   // Admin (can sign/revoke) | Member
    pub issued_at: jiff::Timestamp,
    pub sig: ed25519::Signature,      // by FleetRootKey
}

pub struct Roster {                   // signed, versioned; any host serves the latest it has
    pub fleet: FleetId,
    pub version: u64,
    pub hosts: Vec<HostEntry>,        // { node_id, name, relay_url }
    pub revoked_devices: Vec<iroh::PublicKey>,
    pub sig: ed25519::Signature,
}
```

- **Authentication:** iroh's TLS handshake proves the peer holds the private key for its
  node ID. In the first message (`Hello`) the client sends its `DeviceCert`. The host checks
  the signature against the root, checks that `cert.device` equals the authenticated node
  ID, and checks the revocation list.
- **Roster distribution:** a client that sees a newer roster version pushes it to hosts that
  have an older one. Clients fetch the roster from whichever host they reach first. This is
  the only state that crosses hosts, and every copy is signed by the root, so a host
  can't forge one.
- **Storage on desktop:** keys live in the OS keychain (`keyring` crate: Windows Credential
  Manager, macOS Keychain).
- **Storage in the browser:** the device key and certificate live in IndexedDB on that
  phone. Revoking the certificate from the desktop is the remedy for a lost phone.
- **Trusting the domain:** whoever controls your domain controls the code the phone runs,
  and so could read the device key. Keep the domain's hosting account secured, and add
  Subresource Integrity hashes in `index.html` so only the published WASM build loads.

## 5. Wire protocol (`conductor-proto`)

### 5.1 Connection shape

ALPN is `conductor/1`. Both transports provide independent reliable bidirectional streams,
so we use those instead of hand-rolled multiplexing:

```rust
pub trait Transport {                         // impls: IrohConn (native + wasm), RtcConn (wasm ↔ str0m)
    async fn open_bi(&self) -> Result<(SendHalf, RecvHalf)>;
    async fn accept_bi(&self) -> Result<(SendHalf, RecvHalf)>;
    fn peer(&self) -> PeerId;                 // authenticated iroh node id
    fn path(&self) -> PathKind;               // Direct | Relayed | WebRtc  (shown in the UI)
}
```

On iroh a stream is a QUIC bi-stream. On WebRTC it is an ordered, reliable data channel. The
streams used:

| Stream | Direction | Purpose |
|---|---|---|
| 1st bi-stream | client→host | `Hello { cert, client_version }` → `Welcome { host_info, roster_version, head_seq }` |
| one bi-stream per RPC | client→host | `Command` → `CommandResult`. The stream then closes. |
| one long-lived bi-stream | client→host | `Subscribe { epoch, after_seq }`. The host replays the log from SQLite, then streams live events, or replies `SnapshotRequired` (§8). |

Each frame is a `u32` length followed by a JSON body.

### 5.2 Core types (sketch)

```rust
pub struct HostSeq(pub u64);             // monotonic per host; the cursor
pub struct SessionId(ulid::Ulid);
pub struct CommandId(ulid::Ulid);        // client-generated → idempotent retries
pub struct RequestId(ulid::Ulid);        // approval / question

pub enum Provider { ClaudeCode, Codex }

pub enum Command {
    CreateProject   { path: PathBuf, name: String },
    StartSession    { project: ProjectId, provider: Provider, model: Option<String>,
                      worktree: WorktreeMode, prompt: UserMessage },
    SendMessage     { session: SessionId, message: UserMessage },   // starts, queues or steers
    Interrupt       { session: SessionId },
    ResolveRequest  { request: RequestId, decision: Decision },     // first decision wins
    SetPermissionMode { session: SessionId, mode: PermissionMode },
    RevertToCheckpoint { session: SessionId, checkpoint: CheckpointId },
    ArchiveSession  { session: SessionId },
    GetDiff         { session: SessionId, from: CheckpointRef, to: CheckpointRef }, // read-only
}
pub struct Envelope<T> { pub id: CommandId, pub body: T }

pub struct Event { pub seq: HostSeq, pub at: Timestamp, pub session: Option<SessionId>,
                   pub kind: EventKind }

pub enum EventKind {                      // durable: written to the log, replayed on catch-up
    ProjectCreated(Project),
    SessionCreated(SessionMeta),
    SessionStatus(SessionStatus),         // Idle | Running | AwaitingInput | Errored | Archived
    UserMessage(UserMessage),
    AssistantMessage { turn: TurnId, text: String },     // final text, coalesced
    ToolCall(ToolCall),                   // started/finished; kind: Command|FileEdit|Read|Search|Mcp|Other
    RequestOpened(PendingRequest),        // approval or question, with native option ids preserved
    RequestResolved { request: RequestId, decision: Decision, by: DeviceName },
    TurnCompleted { turn: TurnId, outcome: TurnOutcome, usage: Option<Usage> },
    CheckpointCreated { turn: TurnId, checkpoint: CheckpointId, files_changed: u32 },
    Error(ErrorInfo),
}

pub enum LiveOnly {                       // ephemeral: streamed, never persisted
    AssistantDelta { session: SessionId, turn: TurnId, text: String },
    ToolOutputDelta { session: SessionId, call: ToolCallId, chunk: String },
}
```

The rule: **durable events are the state, deltas are cosmetic.** A client that reconnects
mid-turn misses some deltas, so the host also sends a `TurnSnapshot` with the partial text
so far. T3 Code does the same with `ProviderTextDeltaCoalescer.ts`.

### 5.3 iroh endpoint configuration [verify exact 1.x API names in S3]

- **Relays:** n0's default public relays, with the relay map read from `config.toml` on
  hosts and from settings in clients. Swapping relays is a config change.
- **Discovery:** dial with the address from the roster (`node_id + relay_url + last known
  direct addrs`). Turn on **local-network (mDNS) discovery** on native targets so devices
  on the same LAN connect with no relay. n0's DNS discovery can stay off, because the
  roster already holds every address we need.
- **Browser build:** `iroh` with `default-features = false` for `wasm32-unknown-unknown`.
  In the browser it only ever connects through the relay.
- **Path reporting:** record whether each connection is direct, relayed or WebRTC. The UI
  shows it and S3 measures it.

### 5.4 Browser upgrade to WebRTC

1. The browser connects to the host with iroh-wasm through the relay, then sends `Hello`
   and gets `Welcome`. The browser is now authenticated and the channel is end-to-end
   encrypted.
2. The browser creates an `RTCPeerConnection` (through `web-sys`) and sends an `RtcOffer`
   (its description and connection candidates) over a stream. The host answers using
   `str0m`, a pure-Rust WebRTC library, and the two exchange candidates the same way.
3. **Binding the two channels:** WebRTC's encryption certificates are self-signed. Because
   each side's certificate fingerprint arrived over the authenticated iroh channel, the
   WebRTC connection is bound to the same identities. No extra trust step is needed.
4. **Address-discovery servers:** to learn their public addresses, both sides query a
   public STUN server. These are stateless, see no content, and their list is a setting.
   Nothing relays.
5. Once the data channel opens, new streams use WebRTC and the relayed iroh connection
   idles as a fallback. If the WebRTC connection drops, the client retries the upgrade and
   uses the relay meanwhile.
6. **Reference:** Vibe Kanban `crates/relay-webrtc/` (Apache-2.0, built on `webrtc-rs`) for
   signaling, chunking large messages and host/peer structure.

**This is required, not optional.** You accept n0 relays for setup and for occasional
fallback, but not for all phone traffic, so the phone isn't done until WebRTC carries its
traffic. Build order: relayed path first (it is the fallback anyway), then the upgrade
within the same phase (P3).

**If WebRTC keeps failing for a host,** the phone would sit on the relay for that host:

- The UI shows the path for each host, so this is visible rather than silent.
- The host logs how often the upgrade fails, so we can tell whether it's persistent.
- Remedies, in order:
  1. WSL `networkingMode=mirrored`.
  2. The host asks the router to open a port with UPnP or NAT-PMP, as MonoCode does
     with `igd-next`.
  3. More STUN servers.

  Each one makes the host easier to reach directly.

## 6. Cargo workspace layout

```
conductor/
├── Cargo.toml                    # [workspace], shared deps, lints, edition 2024
├── crates/
│   ├── proto/                    # wire + event types, framing codec. No I/O.
│   ├── trust/                    # fleet keys, DeviceCert, Roster, sign/verify. No I/O.
│   ├── store/                    # SQLite event log, receipts, pending requests, migrations
│   ├── agents/                   # Agent trait + claude/ + codex/ adapters, fixture tests
│   ├── git/                      # worktrees, checkpoints (hidden refs), unified-diff parsing
│   ├── daemon/                   # bin `conductord`: session actors, iroh server, CLI
│   ├── client/                   # client core: host links, cursors, merged fleet view
│   │                             #   compiles to native AND wasm32; platform/ = storage, keys, transport
│   ├── app/                      # Dioxus UI: `dx build --platform desktop` and `--platform web`
│   └── fake-agent/               # test bin that speaks Claude stream-json & Codex JSON-RPC
├── vendor/codex-schema/          # pinned subset of codex app-server JSON Schema (see §7.2)
├── fixtures/                     # recorded real claude/codex stdio sessions for replay tests
├── web/                          # static extras for the domain: manifest.json, icons, sw.js
├── deploy/                       # systemd unit, launchd plist, Task Scheduler XML, WSL notes
└── docs/
```

Dependency direction. Arrows point to dependencies; there are no cycles, and `proto`
and `trust` stay pure:

```
app ──► client ──► proto, trust
daemon ──► agents, git, store ──► proto, trust
agents ──► proto            fake-agent ──► (nothing internal)
```

### 6.1 Crate responsibilities and key dependencies

| Crate | Owns | Key deps (latest on crates.io as of 2026-11-10) |
|---|---|---|
| `proto` | `Command`, `EventKind`, IDs, frame codec | `serde 1.0.229`, `serde_json`, `ulid 3.0`, `jiff 0.2` |
| `trust` | Root/device keys, certs, roster, verification | `ed25519-dalek 3.0`, `iroh` (key types only) |
| `store` | Single-writer append, replay `after_seq`, command receipts, pending requests | `rusqlite 0.41` (feature `bundled`); migrations via `PRAGMA user_version` |
| `agents` | Spawning providers, protocol I/O, mapping to `EventKind` | `tokio 1.54` (process, io-util), `serde_json`; `jsonschema` in tests [verify version] |
| `git` | Create/remove worktrees, per-turn checkpoints, diffs between checkpoints | `tokio::process` calling `git`; a unified-diff parser (`diffy` or our own) [decide] |
| `daemon` | Session actors, iroh router, auth, service install, `pair` QR code, WebRTC answerer, Web Push | `iroh 1.4`, `str0m 0.25` (P3), `web-push 0.11` (P3), `clap 4.6`, `qrcode 0.14`, `tracing 0.1`, `directories 6`, `service-manager 0.11` (launchd, systemd; check its Task Scheduler support), `sd-notify 0.5` |
| `client` | One `HostLink` state machine per host, reconnect with backoff, cursor persistence, merged view | `iroh 1.4` (wasm: no default features), `n0-future` or `futures` for a runtime-agnostic core **[verify]**. Native: `tokio`, `rusqlite`, `keyring 4`. Wasm: `web-sys` (RTCPeerConnection), an IndexedDB wrapper **[pick in S3]** |
| `app` | Screens, components, Markdown rendering, syntax highlighting | `dioxus 0.7.10` (features `desktop`, `web`), `pulldown-cmark 0.13`, `syntect 5.3` (check its wasm build size; switch to a lighter highlighter for web if it is too large) |
| `fake-agent` | Scripted agent for daemon and approval tests at no API cost | `serde_json` |

No relay of our own. The web client is the `app` crate's web build plus `web/`, uploaded
to the domain as static files.

## 7. Agent adapters (`agents`)

**Base:** depend on `claude-codes` and `codex-codes` from crates.io (repository
`meawoppl/rust-code-agent-sdks`, Apache-2.0).

- They already provide the typed protocols, async clients, the initialize handshake,
  interrupts and tool-approval control messages.
- They pin a tested CLI version: Claude 2.1.322 on 2026-11-10, with daily "drift"
  releases.
- Their `types` feature compiles to WASM, so the browser client can share the same
  message types.
- Our `agents` crate is a thin layer on top: one session actor per session, a map that
  makes sure each approval is answered exactly once, and the mapping to our `EventKind`.
- Gaps we find go to `docs/upstream-claude-codes.md` and become upstream pull requests.
  Until a pull request lands, we use `[patch.crates-io]`.

The protocol notes below describe what these crates speak. Use them to review the crates
and to diagnose drift.

```rust
#[async_trait]   // or native async fn in traits + boxed futures
pub trait AgentSession: Send {
    async fn send(&mut self, msg: UserMessage) -> Result<()>;      // start / continue turn
    async fn interrupt(&mut self) -> Result<()>;
    async fn resolve(&mut self, req: RequestId, d: Decision) -> Result<()>;
    async fn set_permission_mode(&mut self, m: PermissionMode) -> Result<()>;
    fn events(&mut self) -> &mut mpsc::Receiver<AgentEvent>;       // normalized; daemon logs them
    fn native_session_id(&self) -> Option<&str>;                    // for resume after restart
}
```

Each adapter also writes **raw provider messages** to a `raw_events` table. That lets us
replay them through a fixed normaliser when we find a mapping bug.

### 7.1 Claude Code

How it works: we have two references.

- **The spec:** Anthropic's MIT-licensed Python SDK (`anthropics/claude-agent-sdk-python`,
  about 1.2k lines in `_internal/query.py` and `_internal/transport/subprocess_cli.py`).
- **An existing Rust implementation to port:** Vibe Kanban
  (`BloopAI/vibe-kanban`, Apache-2.0), `crates/executors/src/executors/claude/{protocol,client,types}.rs`.
  It already sends `initialize`, `interrupt` and `set_permission_mode` and handles
  `can_use_tool` control requests from Rust.

T3 Code and Paseo both use the TypeScript SDK, not the raw protocol. Their option sets
show which SDK features matter:

- `includePartialMessages`, `canUseTool`, `resume`/`sessionId`/`resumeSessionAt`, and
  `forkSession`.
- `settingSources: [user, project, local]`.
- `enableFileCheckpointing`, which `rewind_files` requires.
- The SDK's permission-mode mapping: T3 `ClaudeAdapterV2.ts` `permissionModeForClaudeRuntimePolicy()`
  around line 2804, and Paseo `packages/server/src/server/agent/providers/claude/agent.ts`
  `buildOptions()` around line 3740.

**[verify in S1]** how each SDK option maps to a CLI flag or environment variable.

```
claude --input-format stream-json --output-format stream-json --verbose \
       --permission-prompt-tool stdio --include-partial-messages \
       --permission-mode <mode> [--model M] [--resume <session_id>] [--add-dir ...]
```

- **stdout:** JSON lines. Messages are `system`/init (carries the `session_id`),
  `assistant`/`user` with content blocks, `stream_event` partials, and `result`.
- **Control protocol:** `{"type":"control_request","request_id":..,"request":{"subtype":..}}`
  in both directions, answered with `control_response`.
  - We send: `initialize`, `interrupt`, `set_permission_mode`, `set_model`,
    `rewind_files`, `stop_task`, `get_context_usage`.
  - The CLI sends: `can_use_tool`, which becomes our `RequestOpened` and is answered with
    `{behavior: allow|deny, updatedInput?}`. It may also send `hook_callback` and
    `mcp_message`; we don't need those in v1.
- **Process lifetime:** one long-lived process per active session, with messages streamed
  over stdin. Idle sessions are killed after N minutes and resumed with `--resume` on
  the next message.
- **Pin the CLI version:** the SDK pins one in `_cli_version.py`. Our adapter does the same
  and refuses unknown versions with a clear error. That error tells us when to re-check
  the protocol.
- **T3 Code reference:**
  - `apps/server/src/orchestration-v2/Adapters/ClaudeAdapterV2.ts` (12.5k lines; it uses
    the TS SDK's `query`, `canUseTool`, `interrupt`, `setPermissionMode`, `resume`).
  - `ClaudeTaskOutputTail.ts` for subagent and task output.

### 7.2 Codex

How it works: `codex app-server` speaks JSON-RPC over stdio. Codex is itself Rust (`openai/codex`, `codex-rs/`).

- **Process lifetime:** one app-server process per host serves every Codex thread on that
  host.
- **Framing:** newline-delimited JSON-RPC with no `jsonrpc` field. Allow very long lines:
  Paseo raised its limit to 128 MiB after a long-thread resume reached about 20 MB.
- **Handshake:** `initialize` with `clientInfo` and `capabilities: {experimentalApi: true}`.
- **Methods we use:**
  - Threads: `thread/start` (`cwd`, `model`, `approvalPolicy`, `sandbox`,
    `developerInstructions`), `thread/resume`, `thread/fork`, `thread/rollback`, `thread/read`.
  - Turns: `turn/start`, `turn/steer`, `turn/interrupt {threadId, turnId}`.
- **Notifications we handle:** `turn/started`, `turn/completed`, `turn/diff/updated`,
  `turn/plan/updated`, `item/started`, `item/completed`, `item/agentMessage/delta`,
  `item/commandExecution/outputDelta`, `thread/tokenUsage/updated`.
- **Approval requests (server→client):**
  - Requests: `item/commandExecution/requestApproval`, `item/fileChange/requestApproval`,
    `item/permissions/requestApproval`, `item/tool/requestUserInput`.
  - Decisions: `accept`, `acceptForSession`, `decline` or `cancel`.
  - Permission presets come from Paseo: `on-request`+`read-only`,
    `on-request`+`workspace-write`, and `never`+`danger-full-access`.
- **Types:** Codex checks in JSON Schema for every message
  (`codex-rs/app-server-protocol/schema/json/{,v1,v2}`), and
  `codex app-server generate-json-schema` exports it from the installed binary.
  - We vendor the schema at a pinned Codex tag and **generate** Rust types from it with
    `typify` **[verify in S2 that typify handles it]**. T3 Code does the same in TypeScript:
    `packages/effect-codex-app-server/scripts/generate.ts`, pinned to one upstream commit.
  - Fallback: hand-written serde structs for only the messages we use, plus a test that
    validates fixtures against the schema.
  - A CI job diffs the vendored schema against `generate-json-schema` from the pinned
    Codex version.
  - Don't depend on Codex's crates. The `codex-app-server-protocol` 0.63 on crates.io was
    published by a **third party**, not OpenAI. The upstream git crate pulls in about ten
    internal Codex crates.
- **Rust reference:** Vibe Kanban `crates/executors/src/executors/codex/{client,jsonrpc,normalize_logs}.rs`.
- **Known traps,** from T3 Code's `docs/internals/providers.md` ("Protocol traps"):
  - Codex asynchronous questions arrive as notifications and are answered with a **new user
    message**, not an RPC response. Keep them pending across turns and restarts.
  - `/goal` can start follow-up turns on its own, so one run can own several native turns.
  - Preserve native option IDs for approvals; a display label is not always a valid reply.
- **T3 Code reference:** `apps/server/src/orchestration-v2/Adapters/CodexAdapterV2.ts`,
  `packages/effect-codex-app-server/`.
- **Orchestrator.inc reference:** `backend/internal/adapters/chatdriver/codexappserver/`
  (Go, including generated `codexproto`).

## 8. Daemon internals (`daemon`)

- **Session actor:** one tokio task per session owns its `AgentSession`, worktree and
  turn state. Commands arrive on an `mpsc` channel; there are no locks around session state.
- **Event writer:** a single task appends to SQLite and assigns `seq`, then
  `broadcast`s to subscribers. Every durable event goes through it.
- **Subscribe without gaps:**
  1. Attach to the broadcast first.
  2. Replay rows `> after_seq` from SQLite up to the current head.
  3. Drain the broadcast, dropping `seq <= last_replayed`.

  A subscriber that falls behind on the broadcast is disconnected and re-subscribes from
  its cursor.
- **Idempotent commands:** the receipt for each `CommandId` is stored in the same
  transaction as its effects, so a retried command returns the stored receipt. Copied from
  T3 Code's `CommandReceiptStore.ts`.
- **Requests:** pending approvals and questions live in `pending_requests`. The first
  `ResolveRequest` wins (compare-and-set on `state`). Later ones get `AlreadyResolved` and
  the name of the device that answered.
- **Checkpoints:** at the end of each turn, snapshot the worktree without touching the user's
  index:
  ```
  GIT_INDEX_FILE=<tmp> git add -A
  git write-tree
  git commit-tree
  git update-ref refs/conductor/<session>/<turn>
  ```
  Diffs are `git diff` between checkpoint refs. Revert is `git restore --source=<ref>`,
  combined with Claude `rewind_files` or Codex `thread/rollback` for the conversation.
  This is T3 Code's mechanism: `refs/t3/orchestration-v2/checkpoints/...`, in
  `apps/server/src/vcs/GitVcsDriver.ts` around lines 920–1330. It also handles nested
  repos and sparse checkouts, which we should port.
- **Cursor epochs:** a cursor is `(host_epoch, seq)`. `host_epoch` is random and created
  when the database is, so a wiped or restored database is detected instead of silently
  replaying the wrong history. If the epoch doesn't match, or the requested `seq` has been
  compacted away, the host replies `SnapshotRequired` and the client reloads that host's
  state. The epochs idea comes from Paseo `agent/agent-timeline-store.ts`; the
  snapshot-required reply comes from T3 `packages/contracts/src/orchestrationV2.ts`
  around line 3728.
- **Restart behaviour (v1):** child processes die with the daemon. On startup, mark
  in-flight turns `Interrupted` and resume natively (`--resume` / `thread/resume`) on the
  next message. Later: detached per-session host processes, like Orchestrator.inc's
  `persistenthost`.

SQLite schema (daemon):

```sql
projects(id TEXT PK, path TEXT UNIQUE, name TEXT, created_at)
sessions(id TEXT PK, project_id, provider, native_id, worktree_path, branch, title, status, created_at)
events(seq INTEGER PRIMARY KEY, session_id, at, kind TEXT, payload TEXT)      -- the log
raw_events(id INTEGER PRIMARY KEY, session_id, at, direction, payload TEXT)   -- provider wire
pending_requests(id TEXT PK, session_id, kind, payload TEXT, state, resolved_by, resolved_at)
command_receipts(command_id TEXT PK, result TEXT, at)
roster(version INTEGER PK, blob TEXT)        -- latest signed roster
push_tokens(device TEXT PK, platform, token, updated_at)   -- later
```

## 9. Client core (`client`)

- **Runs on two targets.** The same crate compiles natively (desktop) and to
  `wasm32-unknown-unknown` (browser). Platform differences live in `client/src/platform/`
  behind three small traits: `Storage` (SQLite or IndexedDB), `KeyStore` (OS keychain or
  IndexedDB) and `Connector` (native iroh, or iroh-wasm plus the WebRTC upgrade).
- `HostLink` is one per roster host. Its states are
  `Disconnected → Connecting → Authenticating → CatchingUp → Live`, with jittered backoff.
  It persists the last applied `seq` per host, so cold starts resume from the cursor.
- `FleetState` holds sessions keyed by `(HostId, SessionId)`, folded from events. It has
  no Dioxus types, which keeps it UI-agnostic and unit-testable.
- The local cache (SQLite or IndexedDB) lets both clients show the last known state
  instantly and offline.
- The `app` crate wraps `FleetState` in Dioxus signals. That is the only place the UI
  framework touches the core.

## 10. UI (`app`): what to port from T3 Code (MIT)

We port the designs, not the code. T3's web client (`apps/web/src/components/`) is the
reference, not its React Native app.

| Our screen or component | T3 Code reference | v1? |
|---|---|---|
| Sidebar: hosts → projects → sessions, with status dots | `AppSidebarLayout.tsx`, `ConnectionStatusDot.tsx` | yes |
| Transcript | `ChatView.tsx`, `ChatView.logic.ts` | yes |
| Markdown and code in messages | `ChatMarkdown.tsx` → `pulldown-cmark` + `syntect` | yes |
| Tool-call blocks (command, edit, read, search) | `CommandBlock.tsx`, `chat/` | yes |
| Approval and question panel | the question panels from the "Protocol traps" doc; `ComposerContextPermissions.tsx` | yes |
| Composer | `ComposerPromptEditorTiptap.tsx` → plain multiline input in v1 | yes (simple) |
| Diff panel, per turn and per session | `DiffPanel.tsx` (uses `@pierre/diffs`) → our own renderer from parsed unified diff | yes |
| Branch and worktree toolbar | `BranchToolbar*.tsx` | later |
| Command palette | `CommandPalette*.tsx` | later |
| Terminal drawer | `ThreadTerminalDrawer.tsx` (libghostty WASM) | **no** |
| Preview browser, devices, cloud, guest share, 30 providers, Git hosting integrations | — | **no** |

Phone layout follows `docs/internals/mobile-layout.md` and `mobile-navigation.md` in
T3 Code. The sidebar becomes a stack, and approvals get one-tap actions.

### 10.1 The iPhone web app

- **Installation:** installed with Add to Home Screen. It runs full-screen with its own icon
  (`web/manifest.json`, `display: standalone`), and iOS gives a Home Screen app its own
  storage.
- **Push:** iOS supports standard Web Push for Home Screen web apps, with **no Apple
  Developer account**.
  - We generate our own VAPID key pair (a signing key that identifies us to the push
    service) and keep it on the hosts.
  - The phone registers once, and each host stores the subscription in `push_tokens`.
  - When an approval or question opens, the host sends a content-free push ("desk-wsl:
    approval needed") through Apple's push service with the `web-push` crate.
  - Apple's push service is the one third-party server in the path. It sees only that
    notification.
- **The one handwritten JavaScript file:** `web/sw.js`, about 10 lines. iOS requires a
  service worker to show notifications, and service workers can't be written in Rust.
- **Background:** iOS suspends the web app shortly after it leaves the screen, which drops
  its connections. On return, the client reconnects and catches up from its cursors; this
  is what the cursor/replay design is for. Push covers the time in between.

## 11. Testing strategy

- **Adapter replay tests:** record real `claude` and `codex` stdio sessions into
  `fixtures/`, feed them through the parsers, and assert the exact normalised `EventKind`
  sequence. These fail when a CLI upgrade changes the protocol.
- **Daemon integration tests:** use `fake-agent`, which runs scripted approvals,
  interrupts, crashes and slow output. Assert what a subscribed client sees, including
  reconnect at every `seq`.
- **Pure tests** for `trust` (forged, expired and revoked certificates, wrong node ID) and
  `FleetState` folding.
- **Live smoke test (manual, per release):** real Claude and Codex sessions on a Windows
  host and a WSL host, controlled from the iPhone web app on mobile data. Check the UI's
  path indicator: relayed versus WebRTC.

## 12. Build order

Spikes come first. Each answers a question that could change this document.

| Step | Proves | Done when |
|---|---|---|
| **S1** Claude spike | We can drive `claude` from Rust: approve, deny, interrupt, resume. Build on `claude-codes` | A CLI binary runs a turn, approves one tool call, interrupts the next and resumes the session |
| **S2** Codex spike | Same for `codex app-server` | Same checklist on a Codex thread, plus an async question |
| **S3** Transport and UI spike | Dioxus desktop on Windows and Dioxus web in iPhone Safari (served from GitHub Pages); iroh native and iroh-wasm through n0 relays; hole punching from Windows-native and WSL hosts; **a minimal WebRTC data channel from Safari to a `str0m` host, set up over the relayed iroh connection** | A streaming transcript with an approval button works from the Windows desktop app and from the iPhone Home Screen app on mobile data. We record direct, relayed or WebRTC for each host. WebRTC succeeds from mobile data to at least the Windows and WSL hosts. If the Dioxus web build is unusable in Safari → D2 falls back to Leptos (web) + Tauri (desktop) |
| P1 | One host, desktop app: sessions, worktrees, approvals, log and replay | Daily use on one machine |
| P2 | Fleet key, pairing, roster, multi-host merged view (Windows, WSL, macOS hosts); `conductor host add ssh://…` bootstrap (installs the binary and service, then pairs; design from MonoCode `remote/ssh.rs`) | Two hosts in one sidebar |
| P3 | iPhone web app on GitHub Pages: pairing via `#join=`, Home Screen install, Web Push, **WebRTC upgrade (§5.4)** | An approval request reaches a locked iPhone and is approved from it, and the session traffic runs over WebRTC, not the relay |
| P4 | Checkpoints, diff panel, revert | Review and revert a turn from the phone |
| Later | Planner agent, kanban, PR/CI feedback routing (Orchestrator.inc `lifecycle/`, `observe/scm`), terminal | — |

## 13. Resolved and open questions

Resolved on 2026-11-10:

- **Phone:** iPhone, with no Apple Developer account. The phone client is a Home Screen
  web app with Web Push.
- **Hosts:** Windows (native), WSL and macOS.
- **Relay:** we won't run one. We use n0's public relays for setup and fall back to them
  only when a direct path fails.
- **Web domain:** static files only. It serves the client and nothing else.
- **Desktop:** Windows first, then macOS.
- **UI:** Dioxus.

- **Web hosting:** GitHub Pages, on a custom domain over HTTPS.
- **n0 relays:** acceptable for connection setup and occasional fallback, but **not for all
  of a phone's traffic**. That makes the WebRTC upgrade a requirement (§5.4).

### 13.1 GitHub Pages constraints

- **Deployment:** a GitHub Actions workflow runs `dx build --platform web`, copies `web/`,
  and publishes to Pages. Tagging a release publishes the client.
- **No custom response headers.** That's fine for v1:
  - Dioxus web and iroh-wasm don't need WASM threads, which would need COOP/COEP headers
    **[verify in S3]**.
  - The security policy goes in a `<meta http-equiv="Content-Security-Policy">` tag in
    `index.html`.
- **The web app routes through the URL fragment** (`#/host/...`), so GitHub Pages never
  needs to serve unknown paths. The pairing offer is also in the fragment (§3.1).
- **Integrity:** GitHub Pages serves whatever is in the repository. Protect the deploy
  branch and use Subresource Integrity hashes (§4). The GitHub account is now part of the
  trust boundary, so turn on 2FA.

Still open: nothing blocking. The next step is S1–S3.

## 14. References

| Source | License | Use |
|---|---|---|
| pingdotgg/t3code: `apps/server/src/orchestration-v2/`, `docs/internals/`, `docs/orchestration-v2/`, `apps/web/src/components/` | MIT | Adapter behaviour, event/receipt/checkpoint design, UI designs |
| anthropics/claude-agent-sdk-python: `_internal/query.py`, `transport/subprocess_cli.py` | MIT | Claude control protocol to port |
| openai/codex: `codex-rs/app-server/README.md`, `app-server-protocol/schema/json/` | Apache-2.0 | Codex protocol and schema |
| OrchestratorInc/agent-orchestrator: `adapters/chatdriver/codexappserver`, `lifecycle/`, `observe/scm`, `adapters/workspace/gitworktree` | Apache-2.0 (keep NOTICE) | Codex driver cross-check; later CI/PR routing and worktree edge cases |
| meawoppl/rust-code-agent-sdks: `claude-codes`, `codex-codes` | Apache-2.0 | **Dependencies.** Typed Rust clients for Claude Code and Codex |
| BloopAI/vibe-kanban: `crates/executors/src/executors/{claude,codex}/`, `crates/worktree-manager/` | Apache-2.0 | Second reference for the Claude and Codex clients; worktree manager |
| hardbeat920/monocode: `crates/mobile-link/`, `monocode-core/src/{remote,mobile,provider}/` | MIT | **Rust** phone transport and FFI, SSH host bootstrap, provider lifecycle |
| getpaseo/paseo: `packages/server/src/server/agent/providers/{claude,codex}/`, `protocol/src/{hosts,connection-offer}.ts`, `agent/agent-timeline-store.ts` | Apache-2.0 | Option sets, tool-call mappers, host list, pairing offer, epoch cursors |
| coollabsio/jean: `jean-core/src/projects/` | Apache-2.0 | Adopting existing worktrees |
| n0-computer/iroh | Apache-2.0/MIT | Transport (native and wasm); n0 public relays |
| algesten/str0m | MIT | Host-side WebRTC for the browser upgrade |
| BloopAI/vibe-kanban: `crates/relay-webrtc/` | Apache-2.0 | WebRTC signaling and chunking design |
| DioxusLabs/dioxus | Apache-2.0/MIT | UI |
