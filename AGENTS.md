# Conductor

A Rust orchestrator for Claude Code and Codex sessions across several hosts,
with one web app that installs as an app on the desktop and on the iPhone
Home Screen. The design is in
`docs/architecture.md`; what is built and what is next is in
`docs/roadmap.md`.

## Writing code

- **All code is subset code.** Everything under `crates/` is written in the
  Conductor Rust subset, through the pair loop
  (`.claude/skills/pair/SKILL.md`). The subset, its checker and its
  proposals live in their own repository, `JimLundin/subset`
  (`docs/subset.md` there); `subset.toml` here is Conductor's allowlist,
  and `tools/subset` runs the checker at the pinned revision.
- **The subset grows only by accepted proposals**, made in the subset
  repository. When the subset makes code long, log the friction point and
  write it the long way. Only the owner adds constructs.
- **Gate.** `tools/check.sh` (fmt, clippy for native and wasm, tests,
  `tools/subset check`) passes on every commit.
- **Toolchain.** Nightly, pinned in `rust-toolchain.toml`.
- **One build directory per checkout.** Each worktree builds in its own
  `./target`. Never point worktrees at a shared `CARGO_TARGET_DIR`: cargo
  can then silently build one worktree against another branch's sources.
  Find the directory with `tools/target-dir` (for example
  `"$(tools/target-dir)"/debug/conductord`). Dev builds carry line tables only
  (`Cargo.toml` `[profile.dev]`) to keep each `target/` small.
- **Worktrees are temporary.** Create one per task beside the main
  checkout (`git worktree add ../conductor-<task> -b <branch>`), and remove
  it when the branch is merged or abandoned: `git worktree remove
  ../conductor-<task>` and `git branch -d <branch>`. Scratch worktrees
  outside `~/projects/apps/` get their own `../conductor-target`, so delete
  them and that directory when done.
- **Provider wire types.** `crates/agents/src/{claude,codex}/protocol.rs`
  model only the frames and fields we read or write; everything else stays
  `Unknown`. What the CLIs send is in each adapter's `PROTOCOL.md`.
- **Live checks.** `cargo run --bin s1` (Claude) and `cargo run --bin s2`
  (Codex) drive the real CLIs. Run them after changing an adapter.
- **Commits.** End each commit message with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Open no pull
  requests on anyone's repository.
