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
- **No API is stable.** Every crate is internal to this workspace. Change a
  public API whenever a clearer shape appears, and update its callers in
  the same change; never keep an old shape for compatibility.
- **Gate.** `tools/check.sh [commit|step|release]` runs the standard's
  `rust/check.sh`, which also runs Conductor's suites in `checks.toml`: the
  run-record IP scrub and clippy for the wasm crates at `commit`, and the
  browser suite (`tools/e2e-browser.sh`) at `release`, or at `step` when
  `crates/{app,ui,net,proto,client}` change. Run `commit` before each
  commit and `step` (the default) before merging a task branch; `release`
  adds mutation testing and the slow checks, only in CI or when asked. It
  checks only what changed since the merge base with `main` (`--all` checks
  everything) and reports `cached` for a tree that already passed. Set
  `STANDARDS` when the standards checkout is elsewhere.
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
- **Providers are plugins.** The daemon library (`crates/daemon`) names no
  provider: it takes a `runtime::Plugin` per provider, each provider's
  profile, its sign-in (`agents::SignIn`) and credential forms as data
  (`claude::profile`, `codex::profile`) plus its starter, built by
  `claude_cli::plugin` and
  `codex_cli::plugin`. Credentials are generic form values
  (`proto::Credential`); each provider's core reads its own. Only the
  binary, `crates/conductord`, wires the providers in. Its flags are
  `conductord host --cli <provider>=<path> --model <provider>=<model>`
  (each repeatable, the defaults of that provider's instances), `--home
  <remote>` (the home, `docs/home.md`, stored once given) and `--projects
  <dir>` (repeatable, where it looks for checkouts), and
  `conductord login <instance>`; build it with `cargo build -p
  conductord`. Keep provider-specific code out of `daemon`, `host`,
  `proto`, `client`, `ui` and `app`.
- **Instances.** A host runs instances, not providers: each a named,
  configured use of a provider with its own `InstanceId`, credential,
  private home (`<data>/home/<instance>`), work dir (`<data>/work/<instance>`)
  and agents, kept in the store as `proto::Roster`. Whatever is per
  configured account is keyed by `InstanceId` (the registry, `Welcome`,
  `StartSession`, `SessionCreated`, the store); only what the plugin itself
  describes (`Catalog`: label, note, credential forms) is keyed by
  `Provider`. A fresh host gets one instance per provider, with the
  provider's name as its id.
- **Sign-ins are live only.** A sign-in from the app reaches clients as
  `Feed::SignIn` frames, never as events; its commands keep no receipt;
  and a pasted code (`proto::SignInCode`, whose `Debug` hides it), a
  CLI's sign-in output and its link go to no event, receipt, store or log
  line.
- **Commits.** End each commit message with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Open no pull
  requests on anyone's repository.
