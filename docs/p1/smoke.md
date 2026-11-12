# P1 smoke test

A checklist the owner runs by hand with the real CLIs. It takes about fifteen
minutes and costs a few cents of API use. Tick each box, and write anything
odd under **Findings** with the time it happened, so it can be matched to the
host log.

The automated checks cover the protocol paths (`tools/check.sh`,
`tools/live-host.sh`, `tools/e2e-browser.sh`). This list covers what they
can't: a real phone, its keyboard and Home Screen app, two CLIs running at
once, and how the whole thing feels.

## Setup

- [ ] `main` is pushed, and the Pages deploy for that commit is green, so the
      phone runs the same protocol as the host (ALPN `conductor/3`).
- [ ] Make a scratch git repo: `mkdir -p /tmp/smoke && cd /tmp/smoke && git init`.
- [ ] Start the host from the checkout, with cheap models:

  ```sh
  cargo run -q -p daemon --bin conductord -- host \
    --web-url https://jimlundin.github.io/conductor/ \
    --claude-model haiku 2>host.log
  ```

  It prints the join link and a QR code. Keep `host.log`.

## 1. Pairing

- [ ] Desktop browser: open the join link. The status line says Live.
- [ ] iPhone: scan the QR code with the camera. The app opens and says Live.
- [ ] iPhone Home Screen app: add it if it isn't already there, open it, and
      pair with **Pair** (paste the ticket) or **Change host**. It says Live.

## 2. Claude

In the desktop browser, open **New session**, choose Claude Code, and enter
`/tmp/smoke`.

- [ ] Ask it to create `one.txt` containing `one`. Approve it on the tool
      card. The file exists, and the card says "Approved by browser".
- [ ] Ask it to create `two.txt`. Deny it. No file appears, and the turn ends.
- [ ] Ask for a long answer (for example "explain git rebase in detail").
      Press Interrupt while it streams. The turn ends as interrupted within
      a second or two.

## 3. Codex

Start a second session with Codex in `/tmp/smoke`. Codex's workspace-write
sandbox lets it write inside the session's directory and anywhere under
`/tmp` without asking, so ask for a file in your home directory to get an
approval.

- [ ] Ask it to create `~/smoke-outside/three.txt`. Approve it. The file
      exists.
- [ ] Ask for `~/smoke-outside/four.txt`. Deny it. No file appears.
- [ ] Ask for a long answer and interrupt it once text streams. Codex may
      reason for 15 to 30 seconds before any text appears.

## 4. Both at once, on the phone

- [ ] Send a prompt to each session that needs an approval, without
      answering (for Codex, a file under `~/smoke-outside`).
- [ ] On the iPhone, the session list shows both sessions, each with an
      approval badge.
- [ ] Approve one from the phone and the other from the desktop. Each card
      names who approved it.

## 5. Reload and reconnect

- [ ] Start a long answer, and reload the desktop page while it streams. The
      transcript replays with no duplicated or missing text, and the stream
      continues.
- [ ] Lock the iPhone for 30 seconds and unlock it. The app reconnects on its
      own and shows what happened meanwhile.

## 6. The phone keyboard

- [ ] On the iPhone, tap the composer. The keyboard doesn't cover the
      composer or the last message, and the page doesn't zoom.
- [ ] Type a multi-line message, and send it with the send button.

## 7. Refusals

- [ ] In **New session**, enter a directory that doesn't exist. The form
      shows the rejection, and no session is created.
- [ ] Stop the host, and start it again with `--codex-path /nonexistent`.
      After rejoining, **New session** shows Codex as unavailable, with a
      reason, before you try it.

## Teardown

- [ ] Stop the host with Ctrl-C. The phone shows that the host is gone.
- [ ] `rm -rf /tmp/smoke ~/smoke-outside`.

## Findings

Write one line per finding: the time, the step, and what happened.

### Run 1 (2026-11-12, owner, host on WSL with mirrored networking)

Every step passed. Phone: iPhone, Safari and the Home Screen app, on the same
Wi-Fi as the host.

1. **Step 1, WebRTC never upgrades; every client stays relayed.** Browsers
   offer their LAN address only as an mDNS `.local` name, which the host
   skips (`ICE bad candidate`), and the shared public address needs NAT
   hairpinning, which the router doesn't do. The phone's checks never reach
   the host (0 datagrams in), most likely the Hyper-V firewall blocking
   inbound UDP to WSL. The desktop browser on the same machine exchanges
   datagrams (59 in, 83 out) but ICE stays in Checking; Windows and WSL share
   192.168.0.27 in mirrored mode. The client retries every 15 s, forever.
2. **Step 3, the checklist was wrong**: Codex may write under `/tmp` without
   asking, so `/tmp/smoke-outside` never prompted. Fixed above.
3. **Step 3, Codex shows nothing while it reasons.** The rebase turn sent no
   text for 18 s before the interrupt; the app looked stalled. The interrupt
   itself worked, and the turn ended as interrupted.
4. **Step 4, every client is named "browser"**, so a card can't say which
   device approved. The log showed the phone approved Claude's request and the
   desktop approved Codex's.
5. **Step 6, a gap above the keyboard**: a band about a finger's height
   between the composer and the keyboard's accessory bar, likely the
   home-indicator inset kept while the keyboard is up. Return sends the
   message; the owner accepts that as the design.
6. **Step 7, Codex shows "unavailable" with no reason.** The host sends the
   reason in `Welcome`; the New Session form doesn't render it.
