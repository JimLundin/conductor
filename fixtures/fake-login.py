"""The sign-in stand-in for `claude auth ...` and `codex login ...`.

The first argument names the CLI; the rest are its arguments. It prints what
the pinned CLI prints on pipes (fixtures/<cli>/sign-in.txt, recorded with the
per-run values replaced) and keeps the sign-in in the private home the host
gives it, as `.fake-signed-in`:

- `status` exits 0 if the home is signed in, else 1. Claude's prints what
  its subscription's `auth status` prints, with ACCOUNT as the email unless
  `.fake-account` supplies another value. `.fake-status` can make a signed-in
  probe exit nonzero, die by signal, or stay alive until the host times out.
- A Codex sign-in keeps ACCOUNT in `auth.json`, as the email of the ID
  token its `ChatGPT` sign-in keeps there.
- Claude's `login` reads one line from stdin. The code ACCEPTED signs the
  home in and exits 0; any other prints Claude's refusal to stderr and
  exits 1. A preexisting `.fake-approve` stands in for terminal interaction.
- Codex's `login --device-auth` waits until `.fake-approve` appears in the
  home, as if the person had entered the code on the page, then signs it in.

It writes no code anywhere, so a test can check that none was kept.
"""

import base64
import json
import os
import signal
import sys
import time
from pathlib import Path

ACCEPTED = "conductor-test-sign-in-code#fake-state"
ACCOUNT = "conductor-test@example.invalid"

cli, args = sys.argv[1], sys.argv[2:]
fixtures = Path(__file__).parent
home = Path(os.environ.get("CLAUDE_CONFIG_DIR" if cli == "claude" else "CODEX_HOME", "."))
signed_in = home / ".fake-signed-in"

if cli == "claude" and args == ["setup-token"]:
    if not (sys.stdin.isatty() and sys.stdout.isatty()):
        sys.exit(1)
    sys.stdout.write((fixtures / "claude" / "sign-in.txt").read_text())
    sys.stdout.flush()
    if sys.stdin.readline().strip() != ACCEPTED:
        sys.exit(1)
    print("Long-lived authentication token created successfully!")
    print("Your OAuth token (valid for 1 year):")
    print("\x1b[33msk-ant-oat01-conductor-synthetic-setup-token\x1b[0m")
    print("Store this token securely.", flush=True)
    sys.exit(0)

if args and args[-1] == "status":
    if not signed_in.exists():
        sys.exit(1)
    if cli == "claude":
        account_file = home / ".fake-account"
        email = account_file.read_text().strip() if account_file.exists() else ACCOUNT
        print(json.dumps({"loggedIn": True, "authMethod": "claude.ai", "apiProvider": "firstParty", "email": email or None}))
    sys.stdout.flush()
    status_file = home / ".fake-status"
    status = status_file.read_text().strip() if status_file.exists() else ""
    if status == "nonzero":
        sys.exit(2)
    if status == "signal":
        signal.raise_signal(signal.SIGTERM)
    if status == "timeout":
        time.sleep(60)
    sys.exit(0)

sys.stdout.write((fixtures / cli / "sign-in.txt").read_text())
sys.stdout.flush()
if cli == "claude":
    if not (home / ".fake-approve").exists() and sys.stdin.readline().strip() != ACCEPTED:
        sys.stderr.write((fixtures / "claude" / "sign-in-refused.txt").read_text())
        sys.exit(1)
    print("\nLogin successful.")
else:
    while not (home / ".fake-approve").exists():
        time.sleep(0.1)
    print("Successfully logged in")
    claims = base64.urlsafe_b64encode(json.dumps({"email": ACCOUNT}).encode()).decode().rstrip("=")
    (home / "auth.json").write_text(json.dumps({"tokens": {"id_token": f"e30.{claims}.fake"}}))
signed_in.write_text("signed in\n")
