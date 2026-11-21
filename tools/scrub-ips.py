#!/usr/bin/env python3
"""Replace personal public IP addresses in run records before they are committed.

The test scripts record host logs, which name the host's public address (from
iroh's probes) and sometimes a phone's. Those identify the owner, so every
public address is replaced with one from the documentation ranges (RFC 5737,
RFC 3849), keeping distinct addresses distinct. Private, loopback and
link-local addresses stay.

Usage: tools/scrub-ips.py FILE...          rewrites each file in place
       tools/scrub-ips.py --check FILE...  exits 1 if any file still has one
       tools/scrub-ips.py --check --run-records  checks existing Git run records
"""
import ipaddress
import re
import subprocess
import sys
from pathlib import Path

IP = re.compile(
    r"(?<![\w:])(?P<v6>[0-9a-fA-F:.]*:[0-9a-fA-F:.]*:[0-9a-fA-F:.]*)(?![\w:])"
    r"|(?<![\d.])(?P<v4>\d{1,3}(?:\.\d{1,3}){3})(?![\d.])"
)
RECORDS = (":(glob)docs/**/*.txt", ":(glob)docs/**/*.log", ":(glob)docs/**/NOTES.md")


def run_records():
    listed = subprocess.check_output(
        ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", *RECORDS]
    )
    paths = map(Path, listed.decode(errors="surrogateescape").split("\0"))
    return sorted({path for path in paths if path.is_file() and not path.is_symlink()})


def scrub(text, names):
    def replace(match, make):
        original = match[0]
        trimmed = original.lstrip(".")
        raw = trimmed.rstrip(".")
        prefix, suffix = original[:len(original) - len(trimmed)], trimmed[len(raw):]
        try:
            ip = ipaddress.ip_address(raw)
        except ValueError:
            return match[0]
        mapped = getattr(ip, "ipv4_mapped", None)
        if not (mapped or ip).is_global:
            return match[0]
        return prefix + names.setdefault(raw, make(len(names) + 1)) + suffix

    def addresses(match):
        if match.group("v6") is not None:
            return replace(match, lambda n: f"2001:db8::{n:x}")
        return replace(match, lambda n: f"203.0.113.{n}")

    return IP.sub(addresses, text)


def main():
    names = {}
    if sys.argv[1:2] == ["--check"]:
        paths = run_records() if sys.argv[2:] == ["--run-records"] else sys.argv[2:]
        dirty = [path for path in paths if (original := read(path)) != scrub(original, {})]
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
