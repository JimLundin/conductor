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

Each item ends in something to try. The fleet design is
`docs/architecture.md` §6.

1. **Managed CLIs** (§6.2). `conductord` installs the pinned Claude Code and
   Codex into its data dir, checks their hashes, and runs only those, with
   private `CLAUDE_CONFIG_DIR` and `CODEX_HOME` and updates off.
   `tools/pin` records a pin; a test ties each pin to its fixtures; `s1`
   and `s2` run the pinned copies. Until the app can sign in,
   `conductord login claude|codex` runs the same sign-in flow on the host's
   terminal. *Try:* on a machine with neither CLI installed, start
   `conductord`, sign in, run a session of each, and check that `~/.claude`
   and `~/.codex` are untouched.
2. **A wider session model** (§3): questions to the user, reasoning text,
   approve-for-session, per-session model and effort, resume. Resume runs
   against the private homes from item 1. *Try:* restart a session's CLI
   and continue it.
3. **Sign in from the app** (§6.3), on one host. API keys and subscription
   sign-ins entered on the phone; secrets only in flight in the browser and
   encrypted under the host key on the host. `Welcome` reports each
   provider's sign-in state. *Try:* on a fresh host, sign in to Claude and
   Codex from the iPhone without touching the host's keyboard.
4. **Durability.** The log, receipts and open requests survive a host
   restart (redb + postcard); clients resume from their cursors. Session
   ids become ULIDs (§6.7) before the tables key on them. *Try:* restart
   the host mid-session and continue from the phone.
5. **Worktrees.** One git worktree per session on `conductor/<ulid>`, a
   checkpoint per turn, diffs and revert from the phone. *Try:* revert a
   turn from the phone.
6. **Fleet and pairing** (§6.1, §6.4 to §6.6). A fleet root key signs
   device and host certificates and replaces the shared ticket token;
   `conductord join` and *Add host*; a signed fleet config with the roster,
   pins, settings and API keys, served by any host; per-host subscription
   sign-in after a join; several hosts in one sidebar. *Try:* add a second
   host from the phone and run a session on it with the fleet's key.
7. **Push.** Web Push from the host when an approval or question opens.
   *Try:* lock the phone and get the approval notification.
8. **Move a session** (§6.8), after experiments E4 and E5 (§6.9). Bundle
   the branch, copy the transcript and the events, resume on the target.
   *Try:* move a session from WSL to the Mac between turns and continue it
   from the phone.

## Known bugs

- A fresh connection sometimes gets no `Welcome` and dies at the 30 s idle
  timeout. Not reproduced yet.
