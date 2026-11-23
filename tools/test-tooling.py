"""Controlled regression tests for the run records, pin updater, and live check."""

import multiprocessing
import os
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
child = load("child", "live-host-child.py")
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

    def test_live_host_child_matches_literal_workspace_and_own_parent(self):
        cli = self.scratch / "host.[1]" / "cli" / "codex"
        workspace = self.scratch / "project (a)+[1]"
        for pid, parent, exe, cwd in (
            (101, 70, cli / "v/bin/codex", workspace),
            (102, 71, cli / "v/bin/codex", workspace),
            (103, 70, cli / "v/bin/codex", self.scratch / "project (a)+[2]"),
            (104, 70, self.scratch / "host.1/cli/codex/v/bin/codex", workspace),
        ):
            proc = self.scratch / str(pid)
            proc.mkdir()
            (proc / "stat").write_text(f"{pid} (cli ) name) S {parent} 0")
            (proc / "exe").symlink_to(exe)
            (proc / "cwd").symlink_to(cwd)
        fd = os.open(os.devnull, os.O_RDONLY)
        with patch.object(os, "pidfd_open", return_value=fd, create=True) as opened:
            with patch.object(signal, "pidfd_send_signal", create=True) as sent:
                child.interrupt(self.scratch, 70, cli, workspace)
        opened.assert_called_once_with(101)
        sent.assert_called_once_with(fd, signal.SIGKILL)

    def test_crash_check_selects_the_registered_codex_server_work_directory(self):
        cli = self.scratch / "cli" / "codex"
        work = self.scratch / "work" / "codex-8c2a47d3bdb8d309"
        workspace = self.scratch / "session"
        for directory in (cli, work, workspace):
            directory.mkdir(parents=True)
        args = ["check", "70", str(self.scratch), "codex", str(workspace), "me@example.com"]
        with patch.object(sys, "argv", args), patch.object(child, "interrupt") as interrupt:
            child.main()
        interrupt.assert_called_once_with(Path("/proc"), 70, cli, work)

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
