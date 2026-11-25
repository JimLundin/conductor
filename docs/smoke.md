# P1 smoke test

A checklist the owner runs by hand with the real CLIs. It takes about fifteen
minutes and costs a few cents of API use. Tick each box, and write anything
odd under **Findings** with the time it happened, so it can be matched to the
host log.

The automated checks cover the protocol paths (`tools/check.sh`). This
list covers what they
can't: a real phone, its keyboard and Home Screen app, two CLIs running at
once, and how the whole thing feels.

## Setup

- [ ] Run from the checkout for the green Pages deployment. Check the
      protocol in [the `ALPN` constant](../../crates/link/src/lib.rs).
- [ ] From the checkout, create a fresh run directory and seed a commit.
      Keep the shell variables for restart and teardown.

  ```sh
  mkdir -p target
  smoke_run=$(mktemp -d "$PWD/target/p1-smoke.XXXXXXXX")
  smoke_repo="$smoke_run/repo"
  smoke_outside="$smoke_run/outside"
  mkdir "$smoke_outside"
  git init "$smoke_repo"
  git -C "$smoke_repo" -c user.name="Conductor smoke" \
    -c user.email=smoke@example.invalid commit --allow-empty -m "Seed smoke repo"
  printf '%s\n' "$smoke_repo" "$smoke_outside"
  ```

- [ ] Start the host from the checkout. Sign in or configure credentials for
      this run's host if prompted. For a second host, repeat setup with a
      separate run directory and use a distinct host name and data directory.

  ```sh
  cargo run -q -p cli --bin orchestrator -- host \
    --name smoke-primary --data-dir "$smoke_run/host-data" \
    --web-url https://jimlundin.github.io/conductor/ \
    --model claude=haiku 2>>"$smoke_run/host.log"
  ```

  It prints the join link and a QR code. Keep the log and reuse the data
  directory on restart so the host keeps its identity and session state.

## 1. Pairing

- [ ] Desktop browser: open the join link. The host's group in the list
      says Live.
- [ ] iPhone: scan the QR code with the camera. The app opens and says Live.
- [ ] iPhone Home Screen app: add it if it isn't already there, open it, and
      pair with **Connect** (paste the join link) or **Add host**. It says
      Live.
- [ ] A second host: open its join link, or paste it under **Add host**.
      The list shows both hosts, each under its name, and a new session can
      start on either.

## 2. Claude

In the desktop browser, open **New session**, choose Claude Code, and enter
the absolute path printed for `$smoke_repo`.

- [ ] Ask it to create `one.txt` containing `one`. Approve it on the tool
      card. The file exists, and the card says "Write" with the path and
      "Approved by Windows" (or the desktop's own platform).
- [ ] Ask it to create `two.txt`. Deny it. No file appears, and the turn ends.
- [ ] Ask for a long answer (for example "explain git rebase in detail").
      Press Interrupt while it streams. The turn ends as interrupted within
      a second or two.

## 3. Codex

Start a second session with Codex using the same repository path. Codex's workspace-write
sandbox lets it write inside the session's directory and anywhere under
`/tmp` without asking, so ask for a file under the absolute path printed for `$smoke_outside` to get an approval.
Use a checkout outside `/tmp` for this approval test.

- [ ] With Codex chosen, **New session** says in a muted line that Codex
      writes in this folder and in `/tmp` without asking, and asks before
      writing elsewhere, changing `.git` or using the network.
- [ ] Ask it to create `three.txt` under `$smoke_outside`. Approve it. The card
      shows Codex's reason under the command, if it gave one. The file
      exists.
- [ ] Ask for `four.txt` under `$smoke_outside`. Deny it. No file appears.
- [ ] Ask for a long answer and interrupt it once text streams. Codex may
      reason for 15 to 30 seconds before any text appears; meanwhile the
      transcript ends with "Thinking…".

## 4. Both at once, on the phone

- [ ] Send a prompt to each session that needs an approval, without
      answering (for Codex, a file under `$smoke_outside`).
- [ ] On the iPhone, the session list shows both sessions, each with an
      approval badge.
- [ ] Approve one from the phone and the other from the desktop. Each card
      names the device that approved it: "Approved by iPhone" on one, the
      desktop's platform (for example "Approved by Windows") on the other.

## 5. Reload and reconnect

- [ ] Start a long answer, and reload the desktop page while it streams. The
      transcript replays with no duplicated or missing text, and the stream
      continues.
- [ ] Lock the iPhone for 30 seconds and unlock it. The app reconnects on its
      own and shows what happened meanwhile.

## 6. The phone keyboard

- [ ] On the iPhone, tap the composer. The keyboard doesn't cover the
      composer or the last message, the page doesn't zoom, and no band shows
      between the composer and the keyboard.
- [ ] Type a multi-line message, and send it with the send button.

## 7. Refusals

- [ ] In **New session**, enter a directory that doesn't exist. The form
      shows the rejection, and no session is created.
- [ ] Stop the host, and start it again with `--cli codex=/nonexistent`.
      Repeat the start command with the same `--data-dir` and log append
      redirection. After rejoining, **New session** shows Codex as
      unavailable with a reason.

## Teardown

- [ ] Stop the host with Ctrl-C. The phone shows that the host is gone.
- [ ] Record findings and keep any needed extracts from each host's log.
      After stopping both hosts, inspect the `$smoke_run` path on each
      and remove only the directory created by that shell's `mktemp`
      command. Leave it in place if you still need the logs or state.

## Findings

Write one line per finding: the time, the step, and what happened.

### Run 1 (2026-11-12, owner, host on WSL with mirrored networking)

Every step passed. Phone: iPhone, Safari and the Home Screen app, on the same
Wi-Fi as the host.

1. **Step 3, the checklist was wrong**: Codex may write under `/tmp` without
   asking, so `/tmp/smoke-outside` never prompted. Fixed above.
2. **Step 3, Codex shows nothing while it reasons.** The rebase turn sent no
   text for 18 s before the interrupt; the app looked stalled. The interrupt
   itself worked, and the turn ended as interrupted.
3. **Step 4, every client is named "browser"**, so a card can't say which
   device approved. The log showed the phone approved Claude's request and the
   desktop approved Codex's.
4. **Step 6, a gap above the keyboard**: a band about a finger's height
   between the composer and the keyboard's accessory bar, likely the
   home-indicator inset kept while the keyboard is up. Return sends the
   message; the owner accepts that as the design.
5. **Step 7, Codex shows "unavailable" with no reason.** The host sends the
   reason in `Welcome`; the New Session form doesn't render it.

## Web Push on an iPhone

Use an iPhone on iOS 16.4 or later with Conductor added to the Home Screen.
Deploy the web app over HTTPS with `push.js` at the web app's URL and run a
host built against the same Conductor protocol generation. The host needs
outbound HTTPS to the browser's push service, which is `*.push.apple.com`
for an iPhone. In the Home Screen app, pair exactly one host and wait for
it to reach Live.

1. Open Credentials, tap **Enable notifications**, and allow notifications
   in the iOS prompt. Confirm the app reports **Notifications enabled**.
2. Lock the phone. From a second client, open an approval and leave it
   pending. Confirm the lock-screen notification says only that a session
   needs attention. Tap it and check that the right session shows the open
   request.
3. Repeat with a question. Answer it from the second client before tapping
   the notification, then confirm that the app opens the session and shows
   the answer after it catches up.
4. On a fresh test installation, deny the prompt and check that Conductor
   reports the denial and still opens sessions.
