"""Controlled regression tests for the run records and the pin updater."""

import multiprocessing
import signal
import subprocess
import sys
import tempfile
import unittest
from importlib.machinery import SourceFileLoader
from importlib.util import module_from_spec, spec_from_loader
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parent.parent


def load(name, filename):
    loader = SourceFileLoader(name, str(ROOT / "tools" / filename))
    spec = spec_from_loader(name, loader)
    module = module_from_spec(spec)
    loader.exec_module(module)
    return module


scrub = load("scrub", "scrub-ips.py")
pin = load("pin", "pin")


def change_pin(scratch, provider, version, ready):
    pinned = pin.read()[provider]
    pinned["version"] = version

    def format_stub(_command, **_kwargs):
        return None

    with patch.object(pin.subprocess, "run", side_effect=format_stub):
        ready.wait(timeout=10)
        pin.update(provider, pinned, scratch)


class ToolingTests(unittest.TestCase):
    def setUp(self):
        target = ROOT / "target"
        target.mkdir(exist_ok=True)
        self.scratch = Path(self.enterContext(tempfile.TemporaryDirectory(dir=target))).resolve()

    def test_scrub_compressed_ipv6(self):
        self.assertEqual(
            scrub.scrub(
                "[2606:4700:4700::1111]:443 2606:4700:4700::1111. "
                "[2001:db8::2]:443 [fe80::42%eth0]:80 ::1 fc00::3 ::ffff:192.168.0.1 "
                "12:42:05 h:abc::xyz [::ffff:8.8.8.8]:53",
                {},
            ),
            "[2001:db8::1]:443 2001:db8::1. "
            "[2001:db8::2]:443 [fe80::42%eth0]:80 ::1 fc00::3 ::ffff:192.168.0.1 "
            "12:42:05 h:abc::xyz [2001:db8::2]:53",
        )

    def test_check_includes_new_record_and_skips_deleted_record(self):
        def run(*args):
            return subprocess.run(args, cwd=self.scratch, check=True, capture_output=True)

        run("git", "init", "-q")
        records = self.scratch / "docs/check"
        records.mkdir(parents=True)
        tracked = records / "deleted.txt"
        tracked.write_text("2001:db8::1")
        run("git", "add", str(tracked))
        tracked.unlink()
        (self.scratch / ".gitignore").write_text("ignored.txt\n")
        (records / "ignored.txt").write_text("2606:4700:4700::1111")
        new = records / "new record.log"
        new.write_text("[2606:4700:4700::1111]:443")
        command = [sys.executable, str(ROOT / "tools/scrub-ips.py"), "--check", "--run-records"]
        result = subprocess.run(command, cwd=self.scratch, capture_output=True, check=False)
        self.assertEqual(result.returncode, 1)
        self.assertIn(b"docs/check/new record.log:", result.stdout)
        self.assertNotIn(b"deleted.txt", result.stderr)
        new.write_text("[2001:db8::1]:443")
        run(*command)

    def use_scratch_pins(self):
        path = self.scratch / pin.PINS_RS.name
        path.write_bytes(pin.PINS_RS.read_bytes())
        self.enterContext(patch.object(pin, "PINS_RS", path))
        self.enterContext(patch.object(pin, "ROOT", self.scratch))

    def test_failed_formatter_does_not_replace_pins(self):
        self.use_scratch_pins()
        original = pin.PINS_RS.read_bytes()

        def interrupted(command, **_kwargs):
            Path(command[-1]).write_text("partially formatted")
            raise subprocess.CalledProcessError(-signal.SIGTERM, command)

        with patch.object(pin.subprocess, "run", side_effect=interrupted):
            with self.assertRaises(subprocess.CalledProcessError):
                pin.update("claude", pin.read()["claude"], self.scratch)
        self.assertEqual(pin.PINS_RS.read_bytes(), original)

    def test_concurrent_pin_updates_keep_both_providers(self):
        self.use_scratch_pins()
        ctx = multiprocessing.get_context("fork")
        barrier = ctx.Barrier(2)
        updates = [("claude", "2.0.0"), ("codex", "3.0.0")]
        workers = [
            ctx.Process(target=change_pin, args=(self.scratch, provider, version, barrier))
            for provider, version in updates
        ]
        for worker in workers:
            worker.start()
        for worker in workers:
            worker.join(timeout=15)
            if worker.is_alive():
                worker.kill()
                worker.join()
            self.assertEqual(worker.exitcode, 0)
        self.assertEqual(
            [(provider, pin.read()[provider]["version"]) for provider, _ in updates],
            updates,
        )

    def test_incomplete_platforms_are_rejected(self):
        self.use_scratch_pins()
        original = pin.PINS_RS.read_bytes()
        pinned = pin.read()["claude"]
        del pinned["sha256"]["linux-x64-musl"]
        with self.assertRaisesRegex(SystemExit, "supported platforms: linux-x64-musl"):
            pin.update("claude", pinned, self.scratch)
        self.assertEqual(pin.PINS_RS.read_bytes(), original)

    def test_codex_requires_cosign_before_fetching(self):
        with patch.object(pin.shutil, "which", return_value=None), patch.object(pin, "fetch") as fetch:
            with self.assertRaisesRegex(SystemExit, "cosign is not installed"):
                pin.pin_codex("1.0.0", self.scratch)
        fetch.assert_not_called()

    def test_codex_signature_rejection_fails_closed(self):
        archive = self.scratch / "codex-package-linux.tar.gz"
        assets = {f"{archive.name}.sigstore": {"browser_download_url": "fixture"}}
        failed = subprocess.CompletedProcess([], 1, stderr="signature rejected")
        with patch.object(pin, "fetch", return_value=b"bundle"):
            with patch.object(pin.subprocess, "run", return_value=failed) as run:
                with self.assertRaisesRegex(SystemExit, "signature rejected"):
                    pin.verify_codex_bundle("cosign", "1.2.3", archive, assets)
        command = run.call_args.args[0]
        self.assertIn(pin.CODEX_IDENTITY.format("1.2.3"), command)
        self.assertIn(pin.CODEX_ISSUER, command)


if __name__ == "__main__":
    unittest.main()
