# Conductor

A Rust orchestrator for Claude Code and Codex sessions across several hosts,
with one web app that installs as an app on the desktop and on the iPhone
Home Screen. The design is in
`docs/architecture.md`; what is built and what is next is in
`docs/roadmap.md`.

## Writing code

- **The shared Rust standard.** Code follows the Rust standard in
  the standards project, checked out at
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
- **Existing data is disposable.** Change stored formats without migrations
  or compatibility code, and bump their versions when the encoding changes.
  Apply this policy without asking whether existing data must be preserved.
- **Gate.** `tools/check.sh [commit|step|release]` runs the standard's
  `rust/check.sh`, which also runs Conductor's suites in `checks.toml`: the
  run-record IP scrub, the tooling tests, the e2e fixture check, clippy on
  `crates/web-iroh` for `wasm32-unknown-unknown` (`wasm-clippy`), the
  generated schemas check (`schemas`) and the shared TypeScript standard
  on `web/` (`web`) at `commit`, and at `step` that standard's step with
  the web app's Vitest tests (`web-step`).
  The web app builds with `npm run build` in `web/`, which Pages deploys;
  it first builds `crates/web-iroh`, the only crate built for wasm, into
  `web/web-iroh` with wasm-pack (`npm run pipe`), which needs wasm-pack and
  clang besides the toolchain. Nothing it generates is committed.
- **Generated and recorded files.** `web/src/proto/` is generated from
  `link`'s root types (ADR 0002, ADR 0003) and never edited by hand: after
  changing a type the client link reaches, bump `link::ALPN` and run
  `tools/schemas.sh`
  (needs `npm ci` in `web/`), then commit both. `web/fixtures/feeds/` holds
  host feeds the web app's tests replay, as frozen data: when the client
  link changes, edit them to its new shape.
  Use `commit` for local validation when the environment is available. If
  validation is unavailable or stalls, commit and push the task branch
  with the commands, results and outstanding checks recorded in the commit
  message and handoff. Saving work requires no validation
  waiver. `release` adds mutation testing and the slow checks, in CI.
  It runs as units, each a check of one scope, and skips a unit whose key
  already has a pass recorded on `refs/notes/checks`, which it fetches
  from and pushes to `origin` (`--no-cache` runs every unit). CI,
  `.github/workflows/check.yml`, runs `step` on every pushed branch other
  than `main` and `queue/*`, so only units without a recorded pass run
  there; on a failure its job summary names the failing units with the
  tail of their logs. The standards come from a WSL clone of the
  standards project, `~/projects/tools/standards`, kept fresh by the
  script; set `STANDARDS` to try unpushed standards from another checkout.
- **Landing.** Land a branch with
  `git push origin <branch>:refs/heads/queue/<branch>`, which returns at
  once. `.github/workflows/queue.yml` takes queued branches one at a time
  in push order, rebases each onto `main`, runs `step`, and fast-forwards
  `main` when it passes, then deploys Pages. It refuses a branch that
  changes `.github/workflows` or `.github/actions`, which its token can't
  push; such a branch lands by a direct push to `main` with the owner's
  credentials after `tools/check.sh commit`. Either way it deletes the
  queue ref and sets the commit status `queue` on the pushed commit; read
  it with
  `gh api repos/JimLundin/conductor/commits/<sha>/status --jq '.statuses[] | select(.context == "queue") | [.state, .description, .target_url] | join(" ")'`:
  nothing while it waits, `pending` while it checks, `success` with
  `landed at <sha>`, or `failure` naming the failing units, a conflict, a
  CI change or a moved `main`, linking the run whose summary holds the
  log tails. It needs no secret; no ruleset may block
  `github-actions[bot]` from pushing to `main`.
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
  ../conductor-<task>` and `git branch -d <branch>`. Before removal, run
  `tools/target-dir` in the worktree with the environment used to build it.
  Remove an external build directory only when you can verify that it
  belongs exclusively to that worktree.
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
  `codex_cli::plugin`. A provider is the name its plugin declares
  (`claude::PROVIDER`, `codex::PROVIDER`), and each pins its own CLI
  (`claude::pin`, `codex::pin`). `daemon::start` takes a bound endpoint, those plugins
  and the host's options. `daemon::serve` uses the same entry point for
  `conductord host`. Instances in the stored roster and those added during
  a run use the same plugin launch path. Credentials are generic form values
  (`agents::Credential`); each provider's core reads its own. Only the
  binary, `crates/conductord`, wires the providers in. Its flags are
  `conductord host --cli <provider>=<path> --model <provider>=<model>`
  (each repeatable, the defaults of that provider's instances), `--home
  <remote>` (the home, `docs/home.md`, stored once given) and `--projects
  <dir>` (repeatable, where it looks for checkouts), and
  `conductord login <provider> [--account <account>]`, which picks the
  instance of that account, or without one the provider's own; build it
  with `cargo build -p conductord`. Keep provider-specific code out of `agents`, `daemon`,
  `host`, `install`, `runtime`, `proto`, `link` and the web app.
- **Instances.** A host runs instances, not providers: each one account
  of a provider, keyed by `InstanceId { provider, account }`, with its own
  credential, private home (`<data>/home/<dir>`), work dir
  (`<data>/work/<dir>`, `<dir>` from `host::registry::dir`) and agents,
  kept in the store as `proto::Roster`. The provider names the account,
  not the owner, so an instance is never renamed: a credential's form
  names it, and a sign-in in the provider's own instance (`Account::Own`,
  which every host has) names it once the CLI reports it. Whatever is per
  account is keyed by `InstanceId` (the registry, `Welcome`,
  `Session::Start`, `SessionCreated`, the store); only what the plugin itself
  describes (`Catalog`: label, note, credential forms) is keyed by
  `Provider`.
- **Sign-ins are live only.** A sign-in from the app reaches clients as
  `Feed::SignIn` frames, never as events; its commands keep no receipt;
  and a pasted code (`proto::SignInCode`, whose `Debug` hides it), a
  CLI's sign-in output and its link go to no event, receipt, store or log
  line.
- **Commits.** End each commit message with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Open no pull
  requests on anyone's repository.
