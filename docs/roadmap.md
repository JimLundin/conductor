# Roadmap

What is built and what comes next. Update it in the commit that changes it.

## Built

- **Agents.** `crates/agents` drives the `claude` CLI (stream-json control
  protocol) and `codex app-server` (JSON-RPC): messages, streaming deltas,
  tool calls, approvals, interrupts. `s1` and `s2` are the live checks.
- **Host.** `conductord` serves one host over iroh: an in-memory event log,
  a session actor per agent, one shared Codex app-server. A browser
  upgrades to a direct WebRTC connection (str0m) when it can.
- **Client.** `HostLink` per host: reconnect with backoff, catch-up from the
  cursor, the WebRTC upgrade. `HostView` folds events into what the UI shows.
- **App.** Dioxus screens: session list, new session, session pane, phone
  layout. Browser e2e on the real daemon with fake CLIs.

## Next

1. **Durability.** The log, receipts and open requests survive a host
   restart (redb + postcard); clients resume from their cursors.
2. **Worktrees.** One git worktree per session, a checkpoint per turn,
   diffs and revert from the phone.
3. **Fleet and pairing.** A fleet root key signs device certificates and
   replaces the shared ticket token; several hosts in one sidebar.
4. **Push.** Web Push from the host when an approval or question opens.

## Known bugs

- A fresh connection sometimes gets no `Welcome` and dies at the 30 s idle
  timeout. Not reproduced yet.
