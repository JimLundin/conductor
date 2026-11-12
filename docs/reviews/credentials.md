# Review: gateway credentials (`rv-credentials` at `c81d537`)

Pair loop step 2, roadmap phase 1, item 3 (first part). The diff is
`git diff main rv-credentials`. The friction log is
`.audit/credentials.friction.md`. Every run below used test values
(`conductor-test-*`), the fake CLIs, or `/bin/false`, in scratch data
dirs. No real credential, `~/.claude` or `~/.codex` was read, and
`tools/live-host.sh` was not run. Token counts come from `tools/subset
measure` on this branch, on `main`'s copies, and on scratch copies with a
fix applied. (`tools/subset score <paths>` scores the files under the
config root whatever paths it is given, so it can't compare two copies.)

## Verdict

**Changes requested.** The core is sound. The credentials' `Debug` prints
only the kind, the key stays out of `config.toml`, nothing goes on a
command line, the event log never carries a credential, and the store
uses a fresh 24-byte random nonce from `getrandom` for every seal.
`SUBSET=1 tools/check.sh` passes and `tools/subset check` is clean. Five
fixes are required. Two are leaks: each CLI hands the secret to every
command its agent runs, and from there it can reach the event log. One is
the close reason, one is the store's failure mode, and one is volume. A
sixth is for the docs.

## Required fixes

1. **Claude hands the gateway token to every Bash command.** `command()`
   sets `ANTHROPIC_AUTH_TOKEN` in Claude's environment, and Claude Code
   passes its environment to the subprocesses it starts (Bash, hooks,
   stdio MCP servers) unless `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1` is set.
   The env-vars reference says the scrub is opt-in and that it "reduces
   what a prompt injection attack can read" (code.claude.com/docs/en/env-vars,
   read this session). Failing input: a session on a stored gateway in
   which the agent runs `env` or `printenv ANTHROPIC_AUTH_TOKEN`. That is
   either a request the user approves without reading it, or a prompt
   injection from a file. The tool output, with the token in it, becomes a
   `ToolCall` event. It goes into the log, its replay, every client's DOM,
   and the transcript. This is inferred from the documentation. It is not
   run, because it needs the real CLI. Fix: also set
   `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1` in the `Some(Gateway)` arm, and add
   the name to `fake-claude.sh`'s list so the e2e test asserts it. This
   adds one call, about 8 tokens.
2. **Codex hands `CONDUCTOR_CODEX_KEY` to every shell command.** The
   current config reference says
   `shell_environment_policy.ignore_default_excludes` defaults to `true`,
   which means variables with `KEY`, `SECRET` or `TOKEN` in their names are
   kept (learn.chatgpt.com/docs/config-file/config-reference, read this
   session). The key is in the app-server's environment, so Codex's shell
   tool inherits it. The failing input and the path to the event log are
   the same as in fix 1, with `printenv CONDUCTOR_CODEX_KEY`. Whether the
   pinned 0.166 has the same default is not verified. Fix: have
   `with_provider` write `[shell_environment_policy] exclude =
   ["CONDUCTOR_CODEX_KEY"]` along with the provider, and take it out for
   `None`. This is narrower than `ignore_default_excludes = false`, which
   would also strip the user's own `*_KEY` variables. Extend the
   `config.rs` and e2e expected strings to match. About 30 tokens.
3. **A credential change closes Codex sessions as an `unknown exit`.**
   `Exit::Unknown` is documented as "waiting on the killed process failed".
   The hub uses it here for a stop it chose to make, and it drops the real
   `Exit` that `shutdown` returned into an info log. The user sees "Exited:
   unknown exit (stopped for a new Codex credential)". To the user that
   reads as a crash, but it is the host's own decision, which the host
   knows. Closing those sessions is the right outcome, since the key is
   fixed in the server's environment. Fix: make the close typed. One way
   is a `CloseReason::Stopped { reason }` (or `CredentialChanged {
   provider }`), which the hub's `close_all` sends through its own event,
   so the app words it as "Closed for a new Codex credential" and
   `Exit::Unknown` keeps its one meaning. The ALPN is already bumped in
   this branch, so it costs no extra version.
4. **A store that doesn't open stops the whole host, and a crash can
   cause it.** Observed: with `credentials` holding 18 bytes of garbage,
   `conductord host --data-dir D --claude-path /bin/false --codex-path
   /bin/false` exits with `Error: D/credentials does not open with the
   store key`. The file was left as it was. That is fail-closed, and it
   overwrites nothing, but it is the whole host, not the provider. Both
   providers and every session go offline, and the phone can't reach a
   host that never starts. That misses roadmap item 3's "without
   touching the host's keyboard". Two things cause it:
   - `save` renames `credentials.new` over `credentials` without
     `sync_all`. On a power cut the rename can land before the data, which
     leaves an empty or short file, and that makes the next start fail.
     This is inferred from rename-without-fsync semantics. It is not run.
   - Any corruption, or a `store.key` lost from a backup restore, gives
     the same refusal.
   Fix: call `sync_all` on the file in `write_private` before the rename.
   Then have `Store::open` return the reason instead of failing `host`, so
   the registry reports each provider `Unavailable { reason: "<path> does
   not open with the store key; move it aside" }`. A stored credential
   would have come first, so the host must not quietly fall back to the
   home's sign-in. `save` must refuse with `CredentialNotSaved` while the
   file is unreadable, so a `SetCredential` doesn't overwrite the other
   provider's credential. If the coordinator would rather keep the
   refusal, the minimum is the `sync_all` plus a §6.3 sentence that says
   the host doesn't start and what to move aside.
5. **`WireApi` has one value, so it carries nothing.** It is a type on the
   wire, in the stored file and in the app, and every caller writes
   `WireApi::Responses`. The friction rule is to write the least code for
   the task. Measured saving if it goes, with `wire_api = "responses"`
   written in `with_provider`: 77 tokens and 11 lines across `proto/lib.rs`
   (1977 to 1950), `codex/config.rs` (602 to 579), `app/credentials.rs`
   (1517 to 1508), `headless.rs` (2453 to 2444) and `frame.rs` (1178 to
   1169), plus its uses in `e2e.rs`. A second wire API would be an added
   field, and §6.5's fleet config doesn't exist yet, so nothing has to
   stay compatible with it.

6. **Docs (§6.2, §6.3).**
   - Say that the sign-in check (`claude auth status`, `codex login
     status`) runs once at host start. A `conductord login` while the host
     runs, or a sign-in that expires, shows up only after a restart.
   - "the file variant on every platform" with "mode 0600" isn't true on
     Windows. There `write_private` is `std::fs::write`, which inherits the
     data dir's ACL. That ACL is user-only under the default profile
     directory (inferred), and wider under a `--data-dir` somewhere else.
   - Say that clearing a Claude credential doesn't stop running Claude
     sessions, unlike Codex. For an owner who clears because the token
     leaked, that matters.

## Should fix (not blocking)

- `write_private` went from `create_new` to `create` + `truncate` so that
  `save` can reuse `credentials.new`. That loses two things.
  - `mode(0o600)` applies only when the file is created. A stale
    `credentials.new` at 0644 keeps 0644, and so does the file renamed
    over `credentials`. It holds only ciphertext, and `store.key` stays
    0600.
  - Two first starts on one data dir now both write `store.key` (and
    `host.key`) and both succeed with different keys, where before the
    second failed. The one whose key lost then seals `credentials` under
    a key that is gone, which leads to fix 4.
  `let _ = std::fs::remove_file(&next);` before the write, with
  `create_new` kept, fixes both for about 12 tokens. Inferred from the
  code. The race was not run.
- The format is `nonce || ciphertext`, with no version byte and no AAD.
  Rotation is possible only by trial decryption. §6.3 plans to seal the
  iroh key and the fleet root key under the same host key, and then any
  sealed blob would decrypt as any other. One version byte, also passed as
  AAD with a purpose string (`b"conductor credentials 1"`), costs about 15
  tokens now. Adding it later means a migration.
- `own_sign_in` has no timeout. If `claude auth status` waits on the
  network (not verified), host start hangs on it.
- The browser test checks the input's value, the two storages and the
  URL. §6.3 also promises IndexedDB and no DOM copy. Add
  `!document.documentElement.outerHTML.includes(TOKEN)` and
  `indexedDB.databases()` to the check. Dioxus sets `value` as a property
  (inferred), so this should pass already.
- `base_url` is echoed to every client and written into `config.toml` in
  plain text. A URL with userinfo or a `?key=` would leak through both.
  Rejecting userinfo and a query in `base_url` on the host is cheap.

## What holds

- **Debug and logs.** `ClaudeCredential` and `CodexCredential` print only
  their kind, and every wrapper that holds them derives `Debug` through
  them (`Credentials`, `Credential`, `CommandBody`, `ClaudeLaunch`,
  `ServerOptions`, `Launcher`). `serve.rs` logs `?command`, which shows as
  `SetCredential { credential: Claude(Gateway) }`. No `Command` (whose
  `Debug` would print the environment) is formatted anywhere. Error
  variants (`CredentialNotSaved`, the store's `anyhow` contexts, toml
  errors on a file with no key in it) carry paths and OS errors only. The
  e2e tests count zero test tokens in debug-level logs and in the events,
  and the browser run counts zero in the host log.
- **Command line and files.** Secrets reach the CLIs only through the
  environment, not argv. `headless` reads them from its environment. The
  sealed file is written 0600 at creation, with no window for a fresh
  file. The Codex key never enters `config.toml`, and the e2e test checks
  that.
- **Authorization.** `SetCredential` and `ClearCredential` go through the
  same admitted-client path as every command. A command on the first
  stream is refused, and the e2e test checks that nothing is stored. A
  wrong token is refused the same way, and a refused client gets no
  `Welcome`, so it learns nothing about providers. QUIC hands over the
  lowest stream first, so no command overtakes the `Hello`. Giving Clear
  the same protection as Set is right: the token holder can already run
  sessions, and Clear is idempotent.
- **Restart race.** The hub actor serializes `Start` and `Configure`. A
  thread that starts first is closed by the restart. One that starts
  after runs on the new server. No interleaving runs a new session on the
  old key, and nothing is left half started. `shutdown` is time-bounded,
  so the registry, which awaits it, stays up.
- **Availability order.** `ready` checks for a stored credential first.
  Without one it uses `OwnSignIn`: a signed-in home, an unmanaged
  `--*-path` CLI, or `Missing`, which means "no credential configured".
  That matches §6.3. Clearing the Codex credential takes `model_provider`
  out, so a ChatGPT sign-in takes over again.
- **Log to registry.** The move was needed, because availability now
  changes at run time and the registry owns the credentials. `Log` got 4
  tokens larger.
- **Volume overall.** +5.6k tokens in source and +2.0k in `e2e.rs`. Most
  of it is the registry (+727), proto (+627), the headless `--register`
  (+421) and the new screen (1517). Apart from `WireApi`, nothing is
  speculative. Claude and Codex differ where their CLIs differ (env at
  session start, config plus env at server start), so the two paths
  aren't duplicates.
- **App.** Each field is a wrapping `<label>`. The forms are named by
  their headings. Clear has a full accessible name. Outcomes use `status`
  and errors use `alert`. The copy matches §6.3. The inputs have no `name`,
  so even a submit the browser handled itself would put nothing in the
  URL.

## Risks not tested

- The leaks in fixes 1 and 2 were not run against the real CLIs.
- Password managers. `autocomplete="off"` on a password field is ignored
  by Chrome and Safari, and either may offer to save the token after
  submit. iOS Keychain would then keep it, which goes against "This app
  doesn't keep it". This can't be checked in headless WebKit.
- Core dumps and memory. The daemon doesn't disable core dumps, and
  credentials sit unzeroed in the registry and in the plaintext
  `postcard` buffer. The CLIs hold them in their environments anyway, and
  any session runs as the host user, who can read `store.key`. §6.3
  already says file permissions are the real protection.
- `hello.token == self.token` is a derived, non-constant-time compare.
  This is not new, but the token now also authorizes changing where the
  host's models are reached.
- `close_all` uses `try_send`. A session whose channel is full misses its
  close and stays open in the app. The crash path had this before the
  branch.
- That `claude auth status` and `codex login status` exit non-zero when
  signed out, for the pinned versions.
- Focus after Clear: the button goes away and focus drops to the body.
