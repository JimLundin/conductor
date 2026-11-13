# Roadmap

What is built and what comes next. Update it in the commit that changes it.

## Built

- **Subset v10.** The checker pins `6fb2750`, accepting one shared
  `ask<M, T>` free function in a crate whose actor handles share a stopped
  error type (subset issue #3).

- **Subset v9** (#24) introduced shape-based wire enums, optional fields
  omitted by serde, wire booleans and `@` patterns.
  The adapters use the accepted wire constructs to remove duplicate shapes.
- **Shared adapter types** (#25). Both adapters emit the `proto` tool kinds,
  file changes and outcomes. The daemon keeps request ids, session state and
  approval wording.
- **Agents.** `crates/agents` drives the `claude` CLI (stream-json control
  protocol) and `codex app-server` (JSON-RPC): messages, streaming deltas,
  tool calls, approvals, interrupts. `s1` and `s2` are the live checks.
- **Host.** `conductord` serves one host over iroh: an in-memory event log
  (streaming deltas are live-only), a session actor per agent, one shared
  Codex app-server. Clients reach it over iroh through n0's public relays.
- **Client.** `HostLink` per host: reconnect with backoff, a hedged dial,
  catch-up from the cursor. `HostView` folds events into what the UI shows.
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
   and `~/.codex` are untouched. Built on `managed-cli`; the *Try* waits for
   the owner's sign-in (`conductord login`, then `tools/live-host.sh
   --data-dir`).
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
   Claude shows none (Claude `PROTOCOL.md`). Continue covers CLI restarts
   within a running host; a host restart waits for item 4. Next: plans,
   and the effort changed between turns. *Try:* restart a session's CLI and
   continue it (`tools/live-host.sh` checks it as `continue=pass`).
3. **Sign in from the app** (#12) (§6.3), on one host. Built first, on
   `credentials`: gateway credentials (Claude's `ANTHROPIC_BASE_URL` and
   `ANTHROPIC_AUTH_TOKEN`, a Codex custom model provider) set from the
   app's credentials screen and stored encrypted under `<data>/store.key`;
   `tools/live-host.sh --register` runs the pinned CLIs on them. Next: API
   keys, the Claude `setup-token` token and the Codex device-code sign-in,
   entered on the phone, and the platform keystores for the host key.
   `Welcome` reports each provider's sign-in kind. *Try:* on a fresh host, sign in to Claude and
   Codex from the iPhone without touching the host's keyboard.
4. **Durability.** (#11) The log, receipts and open requests survive a host
   restart (redb + postcard); clients resume from their cursors. Session
   ids become ULIDs (§6.7) before the tables key on them, with
   `CLAUDE_CODE_PROJECT_DIR_NAME` (§6.2), and the store keeps each
   session's conversation id, so `Continue` works after a host restart too.
   *Try:* restart the host mid-session and continue from the phone.
5. **Worktrees.** (#13) One git worktree per session on `conductor/<ulid>`, a
   checkpoint per turn, diffs and revert from the phone. *Try:* revert a
   turn from the phone.
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

## Known bugs

- A fresh connection sometimes gets no `Welcome` and dies at the 30 s idle
  timeout. Not reproduced yet (#6).

Open issues are tracked at https://github.com/JimLundin/conductor/issues.

## Known risks

- n0's free relays now carry all traffic. If n0 ever restricts them, use a
  paid n0 relay or iroh's open-source relay on a small server. The relay URL
  is one setting, so no code changes.
