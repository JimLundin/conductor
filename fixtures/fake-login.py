"""The sign-in stand-in for `claude auth ...` and `codex login ...`.

The first argument names the CLI; the rest are its arguments. It prints what
the pinned CLI prints on pipes (fixtures/<cli>/sign-in.txt, recorded with the
per-run values replaced) and keeps the sign-in in the private home the host
gives it, as `.fake-signed-in`:

- `status` exits 0 if the home is signed in, else 1.
- Claude's `login` reads one line from stdin. The code ACCEPTED signs the
  home in and exits 0; any other prints Claude's refusal to stderr and
  exits 1.
- Codex's `login --device-auth` waits until `.fake-approve` appears in the
  home, as if the person had entered the code on the page, then signs it in.

It writes no code anywhere, so a test can check that none was kept.
"""

import os
import sys
import time
from pathlib import Path

ACCEPTED = "conductor-test-sign-in-code#fake-state"

cli, args = sys.argv[1], sys.argv[2:]
fixtures = Path(__file__).parent
home = Path(os.environ.get("CLAUDE_CONFIG_DIR" if cli == "claude" else "CODEX_HOME", "."))
signed_in = home / ".fake-signed-in"

if args and args[-1] == "status":
    sys.exit(0 if signed_in.exists() else 1)

sys.stdout.write((fixtures / cli / "sign-in.txt").read_text())
sys.stdout.flush()
if cli == "claude":
    if sys.stdin.readline().strip() != ACCEPTED:
        sys.stderr.write((fixtures / "claude" / "sign-in-refused.txt").read_text())
        sys.exit(1)
    print("\nLogin successful.")
else:
    while not (home / ".fake-approve").exists():
        time.sleep(0.1)
    print("Successfully logged in")
signed_in.write_text("signed in\n")
