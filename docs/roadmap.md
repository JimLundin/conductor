# Roadmap

What is built and what comes next. Update it in the commit that changes it.

## Built

- **Agents.** `crates/agents` drives the `claude` CLI (stream-json control
  protocol) and `codex app-server` (JSON-RPC): messages, streaming deltas,
  tool calls, approvals, interrupts. `s1` and `s2` are the live checks.
- **Host.** `conductord` serves one host over iroh: an in-memory event log
  (streaming deltas are live-only), a session actor per agent, one shared
  Codex app-server. A browser upgrades to a direct WebRTC connection (str0m)
  when it can.
- **Client.** `HostLink` per host: reconnect with backoff, catch-up from the
  cursor, the WebRTC upgrade. `HostView` folds events into what the UI shows.
- **App.** One Dioxus web app, installed as an app on the desktop and the
  iPhone: session list, new session, session pane, phone layout. Browser e2e
  on the real daemon with fake CLIs.

## Next

1. **A wider session model** (`docs/architecture.md` §3): questions to the
   user, reasoning text, approve-for-session, per-session model and effort,
   resume.
2. **Durability.** The log, receipts and open requests survive a host
   restart (redb + postcard); clients resume from their cursors.
3. **Worktrees.** One git worktree per session, a checkpoint per turn,
   diffs and revert from the phone.
4. **Fleet and pairing.** A fleet root key signs device certificates and
   replaces the shared ticket token; several hosts in one sidebar.
5. **Push.** Web Push from the host when an approval or question opens.

## Known bugs

- A fresh connection sometimes gets no `Welcome` and dies at the 30 s idle
  timeout. Not reproduced yet.
