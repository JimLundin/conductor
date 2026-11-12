# Review: managed CLIs (`rv-managed-cli` at `31486b5`)

Pair loop step 2, roadmap phase 1, item 1. The diff is
`git diff main rv-managed-cli`. The friction log is
`.audit/managed-cli.friction.md`. Every CLI run below used `env -i` with a
scratch `HOME`, a scratch `CLAUDE_CONFIG_DIR` or `CODEX_HOME`, and a scratch
`--data-dir`. Nothing signed in. The scratch directories are deleted.
Scores come from `tools/subset score` on scratch copies of `install.rs`
checked with `tools/subset check` against this branch's `subset.toml`.

## Verdict

**Changes requested.** The design holds up. The hash is checked before
anything is made executable or unpacked. An installed CLI starts with no
network. The pin test runs without network and catches a version with no
fixture. `tools/subset check` is clean, and `cargo test -p agents -p daemon`
passes. Four fixes are required: two correctness, one test and one volume.
A fifth is for the docs.

## Required fixes

1. **Two installs on one data dir corrupt the binary they "verified".**
   `install` downloads to a fixed `<version>.download` with
   `File::create`, which truncates, and hashes the bytes from the network,
   not the file on disk. Failing input: start `conductord host` on a
   scratch data dir, and once the download passes 250 MB, start a second
   one on the same dir. Observed: the first logs `installed pinned claude
   2.1.316 ... sha256=fd13c700...`, but the file it renamed into place
   hashed `2a6cd625...`. Its `--version` then failed with `Text file busy`,
   because the second still had it open for writing. The second failed
   with `2.1.316.download: No such file or directory`. Both hosts came up
   with Claude unavailable. On macOS there is no `ETXTBSY`, so the first
   host would run the half-zeroed file (inferred). For Codex, the second
   process's `remove_dir_all(dir)` can also delete the first one's
   finished install while it runs. `conductord login` during a first
   `conductord host` start reaches this path. Fix: take
   `std::fs::File::lock` on `<data>/cli/<cli>/lock` around the presence
   check and the install, so the second process waits and then finds the
   CLI present.

2. **The private Claude home still picks up a provider login from the
   environment.** `INHERITED_CREDENTIALS` removes the three token
   variables, but not the provider switches. Failing input: `claude auth
   status` (2.1.316) in an empty `CLAUDE_CONFIG_DIR`, with only
   `CLAUDE_CODE_USE_BEDROCK=1` and `AWS_REGION` set. It reports
   `"loggedIn": true, "apiProvider": "bedrock"`. `CLAUDE_CODE_USE_VERTEX=1`
   and `CLAUDE_CODE_USE_FOUNDRY=1` do the same (observed). Sessions would
   then bill the host's cloud account. Fix: add the three
   `CLAUDE_CODE_USE_*` variables and `ANTHROPIC_BASE_URL`, which would send
   the home's OAuth token to another host (inferred, not tested). Then
   extend the env assertion in `resumed_session_argv` to cover them.

3. **The pin test doesn't check that `pins.rs` is `pins.toml`.** It
   compares only the versions. Failing input: zero the `linux-x64` hash in
   `pins.rs` and leave `pins.toml` alone. `cargo test -p agents --lib pin`
   still passes, 2 of 2. Fix: for each provider, assert that every
   `[<cli>.sha256]` entry in `pins.toml` equals `pins::<cli>_sha256(key)`.
   The test already parses `pins.toml`. This also covers the one way
   `tools/pin` writes a pin it didn't verify. `write` rewrites the other
   provider's section, and regenerates its `pins.rs` arms, from
   `pins.toml` as it stands, so a hand edit there is compiled in without
   a check. Today the 14 hashes agree (checked with a script).

4. **The platform table says each platform twice.** The `Platform` enum
   exists only to be matched again in `names`, and `exe` repeats
   `std::env::consts::EXE_SUFFIX`. Fix: delete the enum. Have one
   `fn platform() -> Result<Names, InstallError>` that matches
   `(OS, ARCH)` straight to `names(claude, codex)`, give `Names` two
   fields, and use `EXE_SUFFIX` in `pin`. This also removes friction
   point 3. A `const` table of `&'static str` rows is not an option. The
   checker rejects it as a keyed lookup and asks for this `match`.
   Measured: the score goes from 2146.5 to 2041.0 (105.5 less), 2248 to
   2113 tokens, and 322 to 300 lines. The subset check is clean. Not
   compiled.

5. **Docs.** §6.2 step 3 says "The previous version stays on disk until no
   session uses it." Nothing removes an old version. Each bump leaves
   about 330 MB for Claude and the unpacked Codex package. Say that old
   versions stay until a later item removes them. The rest of §6.2 and the
   roadmap match what was built. That includes the switch to
   `codex-package-<triple>.tar.gz`, the glibc/musl note, the deferral of
   `CLAUDE_CODE_PROJECT_DIR_NAME` to §6.7, and `conductord login`.

## Not required

- **The `io` closure, friction point 2.** One v-current form exists. Add
  `fn at(path: &Path, done: io::Result<()>) -> Result<(), InstallError>`
  and write `at(dir, std::fs::create_dir_all(dir))?`. It covers 9 of the
  11 sites, since `File::create` and `status()` return values, and an
  unbounded `T` is allowed only for `ask`. Measured: 19 points less
  (2248 to 2226 tokens), and one more line. That's too little to require.
  The friction point stands.
- **Codex scrubbing.** In an empty `CODEX_HOME`, `codex login status`
  says "Not logged in" with `OPENAI_API_KEY` set, and the same with
  `CODEX_API_KEY` (observed). So neither one signs the home in. Whether
  the app-server uses them for requests, or honours `OPENAI_BASE_URL`, was
  not tested. Removing all three in `codex::cli` would match Claude at
  little cost.
- **`tools/pin`.** The GPG check is sound. It uses a fresh `--homedir`,
  requires `IMPORT_OK` for the pinned fingerprint, and requires a
  `VALIDSIG` whose last field, the primary key, is that fingerprint, so
  extra keys in the `.asc` don't count. The fingerprint matches the one
  Anthropic's setup page publishes. Because it is pinned, it doesn't
  matter where the key comes from. For Codex, the `verified` line is
  honest that cosign was skipped. Still, GitHub's digest and
  `codex-package_SHA256SUMS` are uploaded by the same release, so they
  are one source, and macOS and Windows get no signature check even with
  cosign. Saying "no signature checked" in the line would read plainer.
- **Isolation.** Every non-test path that runs a CLI goes through
  `claude::cli` or `codex::cli`: sessions, `--version` and login. Only
  `s1` and `s2` pass `Home::User`. The other `Home::User` sites are the
  replay and e2e tests with fake CLIs. An inherited `CLAUDE_CONFIG_DIR`
  or `CODEX_HOME` is overridden by `cmd.env`. `HTTPS_PROXY` passes through
  to both reqwest and the CLIs, which is what a host behind a proxy needs.
- **The rest of the installer.** A crash during download or `tar` leaves
  a `.download` or `.unpack` file, which the next start truncates or
  removes. The Claude rename is atomic on Linux and macOS. On Windows the
  directory rename is preceded by `remove_dir_all`, since Windows won't
  rename a directory over an existing one. The pinned Linux package has
  no absolute or `..` members (listed), and the hash pins its contents.
  With no network, a present CLI starts normally, and a missing one is
  `Unavailable` with the reqwest error (both run under `unshare -n`).
  Using musl for Codex is right, because it's static, and the doc says so.
  Using glibc for Claude is right except on Alpine, which the doc says is
  not handled. The two `-musl` arms in `pins.rs` are never selected.

## Risks not tested

- Windows and macOS. Nothing tested the directory rename, read-only
  files in `remove_dir_all`, antivirus holding a freshly unpacked file,
  the `tar` resolution order, or the Keychain entry Claude keys to
  `CLAUDE_CONFIG_DIR`. On Windows, a GNU `tar` from Git would read `C:\…`
  as a remote host. Rust searches System32 before `PATH`, which should
  find `bsdtar` first (inferred).
- No connect timeout. `read_timeout` doesn't cover a connect to a
  blackholed route, so the OS default (about 2 minutes on Linux) delays
  the host's start for each missing CLI (inferred).
- Codex warns `Refusing to create helper binaries under temporary dir
  "/tmp"` when `CODEX_HOME` is under `/tmp` (observed). `tools/live-host.sh`
  without `--data-dir` uses `mktemp -d`, so its Codex runs may lack those
  helpers.
- A real sign-in into a private home, and a live session from one. Those
  are the owner's step on the roadmap.
