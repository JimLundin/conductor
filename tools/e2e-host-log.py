"""Remove pairing metadata and tickets from a ready host's diagnostics."""

from pathlib import Path
import re
import sys

log = Path(sys.argv[1]).read_text(errors="replace")
lines = [re.sub(r"^[0-9TZ:.-]+ +", "", line) for line in log.splitlines()]
private = ("ticket:", "join:", "data dir:", "qr image:", "host id:", "relay:")
tickets = {line.removeprefix("ticket:").strip() for line in lines if line.startswith("ticket:")}
tickets.discard("")
for line in lines:
    if line.startswith(private) or any(char in line for char in "\u2588\u2580\u2584"):
        continue
    line = re.sub(r"#join=[^\s\"'<>]+", "#join=<ticket>", line)
    for ticket in tickets:
        line = line.replace(ticket, "<ticket>")
    print(line)
