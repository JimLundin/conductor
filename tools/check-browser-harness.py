"""Regression checks for the browser harness, without building the app."""

from fnmatch import fnmatchcase
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parent.parent
TARGET = ROOT / "target"
BROWSER = ROOT / "crates/app/tests-e2e/browser.ts"


class BrowserHarness(unittest.TestCase):
    def setUp(self):
        TARGET.mkdir(exist_ok=True)
        self.scratch = Path(self.enterContext(tempfile.TemporaryDirectory(dir=TARGET)))

    def test_suite_selection(self):
        with (ROOT / "checks.toml").open("rb") as config:
            suites = tomllib.load(config)["suite"]
        browser = next(suite for suite in suites if suite["name"] == "browser")
        paths = next(rule["paths"] for rule in browser["when"] if rule["tier"] == "step")
        for path in ("fixtures/e2e/claude-approve.jsonl",
                     "crates/claude-cli/tests/fake-claude.sh",
                     "crates/codex-cli/tests/fake-codex.sh"):
            self.assertTrue(any(fnmatchcase(path, glob) for glob in paths), path)
        self.assertFalse(any(fnmatchcase("docs/architecture.md", glob) for glob in paths))

    def test_command_trace_correlation(self):
        lines = """INFO daemon::serve: command received command=123456789012345678901 operation="start_session"
INFO daemon::serve: command finished command=123456789012345678900 outcome="started"
INFO daemon::serve: command finished command=123456789012345678901 outcome="rejected"
INFO daemon::serve: command received command=123456789012345678902 operation="start_session"
INFO daemon::serve: command received command=123456789012345678902 operation="start_session"
INFO daemon::serve: command finished command=123456789012345678902 outcome="started"
INFO daemon::serve: command received command=123456789012345678903 operation="start_session"
INFO elsewhere: command finished command=123456789012345678903 outcome="started"
"""
        for outcome, count in (("started", "1"), ("rejected", "1"), ("accepted", "0")):
            result = subprocess.run(
                ["bash", "-ec", 'source tools/e2e-common.sh; count_browser_commands start_session "$1"', "_", outcome],
                cwd=ROOT, input=lines, capture_output=True, text=True, timeout=5, check=True,
            )
            self.assertEqual(result.stdout.strip(), count)

    def test_locked_install(self):
        package = self.scratch / "crates/app/tests-e2e"
        package.mkdir(parents=True)
        lock = package / "package-lock.json"
        (package / "package.json").write_text("{}")
        (package / "node_modules").mkdir()
        npm = self.scratch / "npm"
        npm.write_text('''#!/bin/sh
[ "$1" = ci ] || exit 2
echo "$1" >> calls
[ "$(cat package-lock.json)" != fail ] || exit 3
''')
        npm.chmod(0o700)
        env = {**os.environ, "PATH": str(self.scratch) + os.pathsep + os.environ["PATH"]}
        command = ["bash", "-c", f'source "{ROOT}/tools/e2e-common.sh"; install_browser_dependencies']
        for content, expected in (("branch-a", 0), ("branch-a", 0), ("branch-b", 0), ("fail", 1)):
            lock.write_text(content)
            result = subprocess.run(command, cwd=self.scratch, env=env, check=False)
            self.assertEqual(result.returncode, expected)
        self.assertEqual((package / "calls").read_text(), "ci\nci\nci\n")
        self.assertFalse((package / "node_modules/.conductor-lock").exists())

    def test_server_ownership(self):
        env = {**os.environ, "target": str(self.scratch)}
        command = ["bash", "-euc", '''
            source tools/e2e-common.sh
            init_browser_scratch
            start_browser_server
            python3 -I -c 'import socket, sys; socket.create_connection(("127.0.0.1", int(sys.argv[1]))).close()' "$port"
        ''']
        with socket.socket() as unrelated:
            unrelated.bind(("127.0.0.1", 0))
            unrelated.listen()
            port = str(unrelated.getsockname()[1])
            failure = subprocess.run(command, cwd=ROOT, env={**env, "PORT": port},
                                     capture_output=True, text=True, timeout=5, check=False)
            self.assertNotEqual(failure.returncode, 0)
            self.assertIn("browser HTTP bind failed", failure.stderr)
            with socket.create_connection(("127.0.0.1", int(port)), timeout=1):
                pass
            self.assertFalse(list(self.scratch.glob("e2e.*")))
            subprocess.run(command, cwd=ROOT, env={**env, "PORT": "0"}, timeout=5, check=True)
            self.assertFalse(list(self.scratch.glob("e2e.*")))

    def test_early_host_failure(self):
        env = {**os.environ, "target": str(self.scratch)}
        command = ["bash", "-euc", """
            source tools/e2e-common.sh
            init_browser_scratch
            (echo 'ticket: synthetic-private-ticket'; exit 23) >"$scratch/host.log" &
            wait_for_join "$!" sample "$scratch/host.log"
        """]
        failed = subprocess.run(command, cwd=ROOT, env=env, capture_output=True, text=True,
                                timeout=5, check=False)
        self.assertNotEqual(failed.returncode, 0)
        self.assertIn("host sample exited before pairing readiness (exit 23)", failed.stderr)
        self.assertNotIn("synthetic-private-ticket", failed.stdout + failed.stderr)
        self.assertFalse(list(self.scratch.glob("e2e.*")))

    def test_failure_diagnostics(self):
        fake = self.scratch / "fake-playwright.mjs"
        fake.write_text('''
const page = {
  on(event, listener) {
    if (event === "console") listener({
      type: () => "error",
      text: () => "console noise synthetic-desk-marker synthetic-laptop-marker",
    });
  },
  async goto(link) {
    if (process.env.E2E_DIAGNOSTICS_MODE === "deadline") {
      return new Promise(() => {});
    }
    throw new Error("navigation refused " + link + " and " + process.env.SECOND_JOIN);
  },
  locator() { return { allInnerTexts: () => new Promise(() => {}) }; },
};
const browser = {
  version: () => "synthetic",
  async newContext() { return { setDefaultTimeout() {}, async newPage() { return page; } }; },
};
export const chromium = { async launch() { return browser; } };
export const webkit = chromium;
''')
        loader = self.scratch / "loader.mjs"
        loader.write_text('''
import { registerHooks } from "node:module";
registerHooks({
  resolve(specifier, context, next) {
    if (specifier === "playwright") {
      return { url: new URL("./fake-playwright.mjs", import.meta.url).href, shortCircuit: true };
    }
    return next(specifier, context);
  },
});
''')
        cmd = ["node", "--import", str(loader), str(BROWSER),
               "http://localhost/#join=synthetic-desk-marker", str(self.scratch / "out.png"),
               "hosts", str(self.scratch)]
        env = {**os.environ, "ENGINE": "chromium", "E2E_DEADLINE": "1000",
               "E2E_PATIENCE": "1000", "SECOND_JOIN": "http://localhost/#join=synthetic-laptop-marker"}
        for setting in ("E2E_DEADLINE", "E2E_PATIENCE"):
            invalid = subprocess.run(cmd, env={**env, setting: "0"}, capture_output=True,
                                     text=True, timeout=5, check=False)
            self.assertEqual(invalid.returncode, 1)
            self.assertIn(f"{setting} must be an integer", invalid.stdout)
            self.assertNotIn("engine:", invalid.stdout)
        for mode, message in (("navigation", "navigation refused"), ("deadline", "scenario took longer")):
            failed = subprocess.run(cmd, env={**env, "E2E_DIAGNOSTICS_MODE": mode},
                                    capture_output=True, text=True, timeout=15, check=False)
            self.assertEqual(failed.returncode, 1, failed.stderr)
            output = failed.stdout + failed.stderr
            self.assertEqual(output.count("last console lines:"), 1)
            self.assertIn(message, output)
            self.assertIn("console noise", output)
            self.assertNotIn("synthetic-desk-marker", output)
            self.assertNotIn("synthetic-laptop-marker", output)


if __name__ == "__main__":
    unittest.main()
