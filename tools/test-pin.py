#!/usr/bin/env python3
"""Check pin generation without fetching a release: python3 tools/test-pin.py."""

import runpy
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PROVIDERS = ("claude", "codex")


class PinTest(unittest.TestCase):
    def setUp(self):
        self.pin = runpy.run_path(str(ROOT / "tools/pin"))
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        scratch = Path(self.scratch.name)
        paths = self.pin["read"].__globals__["PINS_RS"]
        for provider in PROVIDERS:
            copy = scratch / provider / "pins.rs"
            copy.parent.mkdir()
            copy.write_bytes(paths[provider].read_bytes())
            paths[provider] = copy
        self.pin["read"].__globals__["ROOT"] = scratch
        self.original = {provider: self.pin["read"](provider) for provider in PROVIDERS}

    def test_formatted_output_roundtrips(self):
        for provider in PROVIDERS:
            with self.subTest(provider=provider):
                self.pin["write"](provider, self.original[provider])
                self.assertEqual(self.pin["read"](provider), self.original[provider])

    def test_pin_file_read_and_written_back_is_unchanged(self):
        paths = self.pin["read"].__globals__["PINS_RS"]
        for provider in PROVIDERS:
            with self.subTest(provider=provider):
                before = paths[provider].read_bytes()
                self.pin["write"](provider, self.pin["read"](provider))
                self.assertEqual(paths[provider].read_bytes(), before)

    def test_note_with_quotes_and_backslashes_is_escaped(self):
        pinned = {
            "version": "1.0.0",
            "verified": 'the "release" key signs C:\\pins\nand every binary',
            "sha256": {"test-platform": "a" * 64},
        }
        self.pin["write"]("claude", pinned)
        written = self.pin["read"].__globals__["PINS_RS"]["claude"].read_text()
        self.assertIn(r'"the \"release\" key signs C:\\pins\nand every binary"', written)
        self.assertEqual(self.pin["read"]("claude"), pinned)

    def test_each_update_writes_only_its_providers_pin(self):
        for provider, other in (("claude", "codex"), ("codex", "claude")):
            with self.subTest(provider=provider):
                pinned = {
                    "version": "test-version",
                    "verified": "test verification",
                    "sha256": {"test-platform": "a" * 64},
                }
                self.pin["write"](provider, pinned)
                self.assertEqual(self.pin["read"](provider), pinned)
                self.assertEqual(self.pin["read"](other), self.original[other])
                self.pin["write"](provider, self.original[provider])

    def test_malformed_hash_is_rejected(self):
        rs = self.pin["read"].__globals__["PINS_RS"]["claude"]
        digest = next(iter(self.original["claude"]["sha256"].values()))
        rs.write_text(rs.read_text().replace(digest, "bad", 1))
        with self.assertRaises(SystemExit):
            self.pin["read"]("claude")


if __name__ == "__main__":
    unittest.main()
