# Conductor

A Rust orchestrator for Claude Code and Codex sessions across several hosts,
with a desktop app and an iPhone web app. The design is in
`docs/architecture.md`.

## Writing code

- **All code is subset code.** Everything under `crates/` is written in the
  Conductor Rust subset (`docs/subset.md`), through the pair loop
  (`.claude/skills/pair/SKILL.md`). Code from before the subset is being
  rewritten into it, starting with the bootstrap round in `docs/subset.md`.
- **The subset grows only by accepted proposals.** When the subset makes
  code long, log the friction point and write it the long way. Only the
  owner adds constructs.
- **Gate.** `tools/check.sh` (fmt, clippy for native and wasm, tests) and
  `tools/subset check` pass on every commit.
- **Toolchain.** Nightly, pinned in `rust-toolchain.toml`.
- **Vendored crates.** `vendor/claude-codes` and `vendor/codex-codes` are
  our own code now. Fix bugs in place and never contribute upstream
  (`vendor/README.md`).
- **Live checks.** `cargo run --bin s1` (Claude) and `cargo run --bin s2`
  (Codex) drive the real CLIs. Run them after changing an adapter.
- **Commits.** End each commit message with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Open no pull
  requests on anyone's repository.
