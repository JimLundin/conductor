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
- **Gate.** `tools/check.sh` (fmt, clippy for native and wasm, tests) passes
  on every commit. `SUBSET=1 tools/check.sh` also runs `tools/subset check`,
  which becomes mandatory once the rewrite makes `crates/` clean.
- **Toolchain.** Nightly, pinned in `rust-toolchain.toml`.
- **One build directory.** `.cargo/config.toml` points every checkout at
  `../conductor-target`, shared by the main checkout and every worktree
  beside it. Find it with `tools/target-dir` (for example
  `"$(tools/target-dir)"/debug/subset`), never by assuming `./target`.
  Parallel builds take turns on cargo's lock; that is the price of one copy
  of the dependencies instead of one per worktree.
- **Worktrees are temporary.** Create one per task beside the main
  checkout (`git worktree add ../conductor-<task> -b <branch>`), and remove
  it when the branch is merged or abandoned: `git worktree remove
  ../conductor-<task>` and `git branch -d <branch>`. Scratch worktrees
  outside `~/projects/apps/` get their own `../conductor-target`, so delete
  them and that directory when done.
- **Vendored crates.** `vendor/claude-codes` and `vendor/codex-codes` are
  our own code now. Fix bugs in place and never contribute upstream
  (`vendor/README.md`).
- **Live checks.** `cargo run --bin s1` (Claude) and `cargo run --bin s2`
  (Codex) drive the real CLIs. Run them after changing an adapter.
- **Commits.** End each commit message with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Open no pull
  requests on anyone's repository.
