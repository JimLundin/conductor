"""Rerun the daemon e2e scenarios of the committed feeds into an empty directory and compare them byte for byte."""

from pathlib import Path
import os
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
FEEDS = ROOT / "fixtures" / "feeds"


def main():
    committed = {file.name: file for file in FEEDS.glob("*.json")}
    if not committed:
        sys.exit(f"no feeds in {FEEDS}")
    target = ROOT / "target"
    target.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="feeds-", dir=target) as scratch:
        scenarios = sorted(Path(name).stem.replace(".", "::") for name in committed)
        subprocess.run(
            ["cargo", "test", "--quiet", "-p", "daemon", "--test", "e2e", "--",
             "--exact", "--test-threads=1", *scenarios],
            cwd=ROOT, check=True, env={**os.environ, "CONDUCTOR_FEEDS": scratch})
        written = {file.name: file for file in Path(scratch).glob("*.json")}
        differences = committed.keys() ^ written.keys()
        differences.update(name for name in committed.keys() & written.keys()
                           if committed[name].read_bytes() != written[name].read_bytes())
        if differences:
            sys.exit("rerun the scenarios of these feeds: " + ", ".join(sorted(differences)))
        print(f"{len(written)} feeds match their scenarios")


if __name__ == "__main__":
    main()
