#!/usr/bin/env python3
"""Check pin generation without fetching a release: python3 tools/test-pin.py."""

import runpy
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


class PinTest(unittest.TestCase):
    def setUp(self):
        self.pin = runpy.run_path(str(ROOT / "tools/pin"))
        self.original = self.pin["read"]()
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.rs = Path(self.scratch.name) / "pins.rs"
        self.rs.write_bytes((ROOT / "crates/agents/src/pins.rs").read_bytes())
        self.pin["read"].__globals__["PINS_RS"] = self.rs
        self.pin["read"].__globals__["ROOT"] = self.rs.parent

    def test_formatted_output_roundtrips(self):
        self.pin["write"](self.original)
        self.assertEqual(self.pin["read"](), self.original)

    def test_each_update_preserves_the_other_provider(self):
        for provider, other in (("claude", "codex"), ("codex", "claude")):
            with self.subTest(provider=provider):
                pins = self.pin["read"]()
                pins[provider] = {
                    "version": "test-version",
                    "verified": "test verification",
                    "sha256": {"test-platform": "a" * 64},
                }
                self.pin["write"](pins)
                updated = self.pin["read"]()
                self.assertEqual(updated[provider], pins[provider])
                self.assertEqual(updated[other], self.original[other])
                self.pin["write"](self.original)

    def test_malformed_hash_is_rejected(self):
        text = self.rs.read_text()
        digest = next(iter(self.original["claude"]["sha256"].values()))
        self.rs.write_text(text.replace(digest, "bad", 1))
        with self.assertRaises(SystemExit):
            self.pin["read"]()


if __name__ == "__main__":
    unittest.main()
