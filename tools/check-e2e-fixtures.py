"""Regenerate the E2E fixtures in an empty output directory and compare them byte for byte."""

from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
FIXTURES = ROOT / "fixtures"
BUILDERS = ("build-claude.py", "build-codex.py")

def main():
    target = ROOT / "target"
    target.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="e2e-fixtures-", dir=target) as scratch:
        generated = Path(scratch) / "fixtures"
        shutil.copytree(FIXTURES, generated, ignore=shutil.ignore_patterns("e2e"))
        output = generated / "e2e"
        output.mkdir()
        for builder in BUILDERS:
            script = output / builder
            shutil.copyfile(FIXTURES / "e2e" / builder, script)
            subprocess.run([sys.executable, "-I", script], check=True)
        committed = {file.name: file for file in (FIXTURES / "e2e").glob("*.jsonl")}
        recordings = {file.name: file for file in output.glob("*.jsonl")}
        differences = committed.keys() ^ recordings.keys()
        differences.update(name for name in committed.keys() & recordings.keys()
                           if committed[name].read_bytes() != recordings[name].read_bytes())
        if differences:
            sys.exit("regenerate E2E fixtures: " + ", ".join(sorted(differences)))
        print(f"{len(recordings)} generated recordings match")


if __name__ == "__main__":
    main()
