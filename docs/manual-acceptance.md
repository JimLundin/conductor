# Conductor manual acceptance

Use this guide after the local checks pass and the matching web app is
deployed over HTTPS. A is the WSL host, B is a native Mac, and the phone
is an iPhone with the app installed on its Home Screen. Use disposable
accounts and browser profiles. Set up each host and the phone once,
then reuse them through the checks below.

Record the date, app and daemon commit, pinned provider versions,
OS/architecture, browser/iOS version, and whether each check passed,
failed or was blocked. Use test host aliases and synthetic prompts.
Keep join links, invitations, codes, tokens, account identifiers, push
endpoints, vault contents and raw CLI output out of shared evidence.

## Set up the hosts

On A and B, open a Bash shell at the root of the same checkout. Use a
disposable OS profile and verify the deployed app uses the checkout's
`ALPN` in [proto](../crates/proto/src/lib.rs). Set `app_url` to the actual
deployment URL. Each host gets its own run directory.

```sh
umask 077
mkdir -p target
run_dir=$(mktemp -d "$PWD/target/conductor-manual.XXXXXXXX")
mkdir "$run_dir/claude" "$run_dir/codex"
export CLAUDE_CONFIG_DIR="$run_dir/claude"
export CODEX_HOME="$run_dir/codex"
git init "$run_dir/repo"
git -C "$run_dir/repo" -c user.name="Conductor acceptance" \
  -c user.email=acceptance@example.invalid commit --allow-empty -m "Seed acceptance repo"
cargo build --locked -p conductord
conductor_bin="$(tools/target-dir)/debug/conductord"
```

For a task branch ahead of the Pages deployment, build the matching web
app on A. Serve its public files at `/conductor/` on an HTTPS origin the
iPhone trusts. With an existing HTTPS reverse proxy, use this local
backend and route that path to the port it prints.

```sh
(cd crates/app && dx build --platform web --release)
mkdir "$run_dir/site"
ln -s "$(tools/target-dir)/dx/app/release/web/public" "$run_dir/site/conductor"
python3 -I tools/e2e-http.py 0 "$run_dir/site"
```

Keep this server running in a separate terminal and set `app_url` to
the HTTPS URL including `/conductor/`. Only the generated public site
belongs in the served directory.

Set `host_name` to a different alias on each machine. Start this on A
first and keep the terminal private. Omit `--cli` to test the managed
installer.

```sh
"$conductor_bin" host --data-dir "$run_dir/host" --name "$host_name" --web-url "$app_url"
```

Pair the iPhone from A's join link. Use the synthetic repository for
directory sessions. For issue handoff, first set up a disposable home
remote and registered project as described in [the home guide](home.md).
Record the absolute paths of the pinned Claude and Codex executables
installed in the disposable host data, and set `claude_bin` and
`codex_bin` to those paths in the test shells. Use the pinned copies
for the raw CLI checks.

## Account experiments (#2)

Tell the owner before **each E1 through E5 run**. Use explicit scratch
`CLAUDE_CONFIG_DIR` and `CODEX_HOME` throughout. Keep the owner's normal
`~/.claude` and `~/.codex` out of every read and write. These experiments
can consume quota or invalidate a test login. Record the results and any
design changes against [architecture §6.9](architecture.md#69-experiments).

1. **E1, pipes.** On A, run the helper twice. Each final scratch
   directory must be new or empty and owner-only. Follow the displayed
   authorization URL on the second device and enter the response code
   into the helper's hidden prompt. Keep the optional token output private
   if you need it for E2.

   ```sh
   python3 -I tools/claude-e1.py "$claude_bin" "$run_dir/e1-setup" setup-token --save-token token
   python3 -I tools/claude-e1.py "$claude_bin" "$run_dir/e1-login" login
   ```

   Record whether both succeed with pipes. If either fails, record the
   error category before trying it in a fresh scratch profile with a PTY.
   To sign out the fallback test login, use the helper's nested
   `$run_dir/e1-login/home` and `$run_dir/e1-login/claude` paths with the
   pinned CLI so the command reaches that login.

2. **E2, four days.** On A and B, put the same disposable setup token in
   `CLAUDE_CODE_OAUTH_TOKEN` through a private environment and run a short
   synthetic turn on both at the same time each day. Keep a separate pair
   of scratch configs for independent `claude auth login --claudeai`
   logins to the same test account. Run a turn on each daily and compare
   `claude auth status` privately after a refresh. Record the two hosts'
   outcomes for all four days. Leave the refresh observation open if you
   never see a refresh. A second login may invalidate the first.

3. **E3, day 1 and day 9.** Sign into the same disposable ChatGPT account
   independently on A and B using the pinned Codex device-code command.
   Run the refresh command on A, then on B. Run the test turn on both
   after the refreshes, and repeat the sequence on day nine.

   ```sh
   "$codex_bin" login --device-auth
   python3 -I docs/verification/codex_manual_probe.py "$codex_bin" "$run_dir/repo" refresh
   python3 -I docs/verification/codex_manual_probe.py "$codex_bin" "$run_dir/repo" turn
   ```

   The probe calls `account/read` with `refreshToken: true` and prints a
   result without account fields. Separately record whether the historical
   copied-login comparison yields `refresh_token_reused`. The comparison
   is account-disruptive and remains an owner-controlled check on a
   separate disposable account after the independent observations. Do
   not automate credential copying or attempt it on an owner account.

4. **E4, native Claude resume.** On A use a single synthetic Claude
   session with a fresh `CLAUDE_CODE_PROJECT_DIR_NAME`. Give it a harmless
   word to remember, then have a second turn read and edit a synthetic
   file. Copy the test branch and only that project's transcript directory
   into B's scratch Git checkout and Claude config. Keep the same project
   directory name and the original native transcript bytes. Run the same
   pinned Claude on B from the different checkout path with
   `--print --verbose --input-format stream-json --output-format stream-json --resume <session-id>`.
   Send a user frame in the [Claude wire format](../crates/claude/PROTOCOL.md)
   asking for the word and a relative-path edit. Record whether the history
   and edit use B's checkout. Keep any stdout within the private run area.

5. **E5, native Codex resume.** On A, use the probe to create a test thread
   and two turns. After it exits, copy the test branch and the single
   rollout belonging to the printed thread ID. Put the rollout at the
   same path relative to B's scratch `CODEX_HOME` under
   `sessions/YYYY/MM/DD/`. Keep the target repo's absolute path different.
   Run the resume probe on B and enter the disposable thread ID when
   prompted. The helper sends the ID and B's `cwd`, then checks remembered
   history and a new file in B's repository. Record whether both passed.

   ```sh
   # A
   python3 -I docs/verification/codex_manual_probe.py "$codex_bin" "$run_dir/repo" seed
   # B, after copying the rollout and branch
   python3 -I docs/verification/codex_manual_probe.py "$codex_bin" "$run_dir/repo" resume
   ```

## Phone sign-in, native install, and vault (#12, #20)

On the phone, open **Credentials** and sign in to Claude and Codex.
Exercise Claude setup-token, the local Claude fallback, Codex device code,
and the direct Anthropic and OpenAI API-key forms. Follow the app's
provider security-setting prompt if required. Start a session from each
configured instance. The managed CLIs should run on a host that had
neither CLI installed beforehand. In the disposable profile, verify its
previously absent default CLI directories remain absent. A terminal
fallback uses `conductord login claude --data-dir <dir>` or
`conductord login codex --data-dir <dir>`, with `--account` only for a
named instance.

Repeat the native installer on disposable Windows and macOS users.
Interrupt one in-flight download or extraction, retry with the same
data, and confirm the expected pin launches from its private home.
Record OS/arch, stage, recovery and model session outcome. Avoid
touching an existing managed install. On each available native vault
platform, follow [credential store migration and recovery](credential-store.md)
with synthetic credentials and a private matching backup. Verify vault
use, roster preservation, `store.key` retirement after reopen, and
matched-key recovery only in a backup clone. WSL file fallback and native
Linux Secret Service are separate observations. Use only provider-confirmed
controls for token revocation and record any unsupported case.

## Fleet and locked-phone push (#14, #15)

While the iPhone has a single standalone pairing, run the
[Web Push steps](p1/smoke.md#web-push-on-an-iphone) on iOS 16.4 or later.
Check a pending approval, a question already answered on desktop before
tapping, and permission denial in a fresh installation. Record the
notification's generic content, destination and current request status.

After E2/E3, [set up the fleet](fleet.md#set-up-a-fleet) with A as root.
Use A's actual relay URL and keep the sealed backup separate. Pair the
phone with the fleet and use **Add host** to invite B. Run the displayed
join command on B with B's data dir, then start B with that same dir.
Verify both hosts appear and B runs a session using a shared test key or
Claude token without re-entering it. Sign into Codex independently on B.

Use `conductord fleet --data-dir <dir> status` to compare versions.
Take B offline, make a benign signed revision on A, reconnect B and
verify it catches up. Verify a synchronized member can serve with the
root offline. A restarting isolated member should wait for authenticated
sync. Revoke a disposable browser first and B only after the move
checks. Follow the [revocation and restore steps](fleet.md#availability-and-revocation)
and verify the revoked peer cannot reconnect. Rotate distributed test
secrets before retiring the revoked member. Restore root only in a
replacement test directory with the original root stopped.

## Handoffs, subagents, and moves (#30, #31, #16, #115)

- **Tasks.** Open Claude and Codex sessions on the same host. On the phone,
  use **Hand off** on an unfinished Claude task and select the idle Codex
  session. Verify it leaves Claude, appears once in Codex with its
  identity intact, and is completed. Record whether the receiver needed
  a follow-up prompt.
- **Subagents.** In separate real Claude and Codex sessions, ask each to
  delegate two distinct harmless jobs. Open both child transcripts on
  the phone. Record start/end, type, tools, approval attribution,
  checklist and result as the real provider supplies them. Check parent
  checklist isolation after reload. Missing provider events are a result.
- **Move.** After E4/E5, give A and B matching CLI pins, credentials, and
  copies of a synthetic project at different paths. In a session on A
  make two checkpoints plus a harmless uncommitted edit. Between turns
  choose **Move session**, then B. Verify one session on B with the
  original history, prior **Changes**, preserved edit, and a new edit
  made on B. Undo an eligible pre-move turn on B and check both files
  and conversation history. Check that a move with a running turn or
  open request is refused or settled as the UI indicates.
- **Recovery.** Use separate throwaway sessions to stop the target
  during transfer and restart the source with a transfer pending.
  Restart using the same data directories, then use **Retry move** if
  shown. If the move completed before the stop, record that attempt as
  inconclusive. Confirm one owner, usable history and earlier diffs. A
  refused move should leave the source usable. Test mismatched pins only
  with a coordinator-prepared, verified alternate pin that has not already
  been synchronized away. Restore matching pins before retesting success.
- **Finish.** For a separate issue session, select **Finish**, check its
  transcript says the branch was pushed, and start a new session on B
  from a checkout that can fetch the branch. Record any local-only or
  push-failed outcome separately from cross-host session movement.

## One registered live-host run per provider (#4, #9, #121, #122)

Only run this if final acceptance needs real gateway and crash evidence.
Use Linux with pidfd support, one disposable gateway profile per provider,
and a private terminal. The Codex registration harness reads
`~/.codex/config.toml`; it must refer to a **disposable OS profile's**
synthetic configuration. If the host has no such profile, leave the
registered Codex live check open. Configure only the test provider
credential described by the script; keep all values off the command line.
Use the documented `--model` option when discovery advertises the
requested choice. Omitting it preserves the provider's credential-selected
default and the documented cheap host defaults.

```sh
tools/live-host.sh --provider claude --register --undo "$run_dir/live-claude"
tools/live-host.sh --provider codex --register --undo "$run_dir/live-codex"
```

Record the non-sensitive `RESULT` fields, including gateway identity and
crash/Continue verdict. The harness verifies the selected instance for
every scenario and interrupts only the run-owned provider process.
Replay and synthetic process-ownership fixtures belong to local
verification. Do not create a second real host or rerun the crash in a
loop solely to repeat those fixtures.

## Local evidence and cleanup

For #26, fixture regeneration, IP scrub, Chromium/WebKit and the
worktree smoke run use disposable local build artifacts. Confirm the
browser run does not modify tracked docs and leaves reports under the
checkout build directory. This needs neither real accounts nor a phone.
See [verification boundaries](verification.md) for the local suites.

Preserve the run dirs until the four-day and day-nine observations and
any move diagnosis are complete. Then stop only the processes started
here, sign out the test accounts, unpair disposable browsers, remove
only proved test vault entries and verify the exact run directory before
removing it. Keep any retained redacted evidence and sealed backups in
private storage.
