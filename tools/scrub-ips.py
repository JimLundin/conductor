#!/usr/bin/env python3
"""Replace personal public IP addresses in run records before they are committed.

The test scripts record host logs, which name the host's public address (from
STUN or iroh's probes) and sometimes a phone's. Those identify the owner, so
every public address is replaced with one from the documentation ranges
(RFC 5737, RFC 3849), keeping distinct addresses distinct. Private, loopback
and link-local addresses stay, and so do the public servers we talk to on
purpose (the STUN servers), which identify nobody.

Usage: tools/scrub-ips.py FILE...          rewrites each file in place
       tools/scrub-ips.py --check FILE...  exits 1 if any file still has one
"""
import ipaddress
import re
import sys

# Public servers named in our logs on purpose. Keeping them keeps the logs
# readable; they identify nobody.
KEEP = {
    "74.125.250.129",  # stun.l.google.com
    "162.159.207.0",  # stun.cloudflare.com
}

V4 = re.compile(r"(?<![\d.])(\d{1,3}(?:\.\d{1,3}){3})(?![\d.])")
V6 = re.compile(r"(?<![0-9A-Fa-f:])((?:[0-9A-Fa-f]{1,4}:){2,7}[0-9A-Fa-f]{1,4})(?![0-9A-Fa-f:])")


def scrub(text, names):
    def replace(match, make):
        raw = match.group(1)
        try:
            ip = ipaddress.ip_address(raw)
        except ValueError:
            return raw
        if not ip.is_global or raw in KEEP:
            return raw
        return names.setdefault(raw, make(len(names) + 1))

    text = V4.sub(lambda m: replace(m, lambda n: f"203.0.113.{n}"), text)
    return V6.sub(lambda m: replace(m, lambda n: f"2001:db8::{n:x}"), text)


def main():
    names = {}
    if sys.argv[1:2] == ["--check"]:
        dirty = [path for path in sys.argv[2:] if scrub(read(path), {}) != read(path)]
        for path in dirty:
            print(f"{path}: has a public IP address; run tools/scrub-ips.py on it")
        sys.exit(1 if dirty else 0)
    for path in sys.argv[1:]:
        with open(path, encoding="utf-8", errors="surrogateescape") as f:
            text = f.read()
        with open(path, "w", encoding="utf-8", errors="surrogateescape") as f:
            f.write(scrub(text, names))


def read(path):
    with open(path, encoding="utf-8", errors="surrogateescape") as f:
        return f.read()


if __name__ == "__main__":
    main()
