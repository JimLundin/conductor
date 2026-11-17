# Conductor

A Rust orchestrator for Claude Code and Codex sessions across several hosts,
with one web app that installs as an app on the desktop and on the iPhone
Home Screen. The design is in
`docs/architecture.md`; what is built and what is next is in
`docs/roadmap.md`.

## Writing code

- **The shared Rust standard.** Code follows the Rust standard in
  `C:\Users\Jim.Lundin\Projects\standards` (`README.md` there): pedantic
  clippy with no suppressions, libraries as a pure `no_std` core with a
  data-only API, and binaries and shell crates as a thin shell. A crate that
  does I/O declares `layer = "shell"` under `[package.metadata.standards]`
  in its `Cargo.toml`; its decisions belong in a core crate. Relax a check
  only with a reasoned entry in `exceptions.toml`; never with an `#[allow]`
  or a weaker config.
- **Gate.** `tools/check.sh` (the standard's `rust/check.sh`, the run-record
  IP scrub, and clippy for the wasm crates) passes on every commit. Set
  `STANDARDS` when the standards checkout is elsewhere. `tools/check.sh
  full` adds mutation testing and the slow checks.
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
- **Provider wire types.** `crates/{claude,codex}/src/protocol.rs` model
  only the frames and fields we read or write; everything else stays
  `Unknown`, and each crate maps its events into the provider-agnostic
  `crates/agents`. The processes run in `crates/{claude,codex}-cli`. What
  the CLIs send is in each wire crate's `PROTOCOL.md`.
- **Commits.** End each commit message with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Open no pull
  requests on anyone's repository.
