# claude-codes

Typed bindings for the [Claude Code](https://docs.anthropic.com/en/docs/claude-code)
CLI's stream-json protocol. Conductor vendors this crate and maintains it as its own
code; see `../README.md`.

## What it provides

- `io`: `ClaudeInput` (stdin frames) and `ClaudeOutput` (stdout frames), including the
  control protocol (`initialize`, `interrupt`, `set_permission_mode`, `can_use_tool`).
  Unknown frame types and control subtypes parse into `Unknown` variants that keep the
  raw JSON.
- `cli`: `ClaudeCliBuilder`, which builds the `claude` command line.

The crate does not run the CLI. The caller spawns the built command and owns its stdio.
Conductor's session actor in `crates/agents/src/claude/` does this.

## Feature flags

| Feature | Description | WASM-compatible |
|---------|-------------|-----------------|
| `types` | Message types only | Yes |
| `cli` | `ClaudeCliBuilder` (adds tokio) | No |
| `auth` | PTY-driven `claude auth login` and `setup-token` | No |

`types` and `cli` are on by default.

## Login tooling (`auth` feature)

The CLI's login flows are interactive Ink TUIs (they hang on a pipe), so the
`auth` module drives them under a pseudo-terminal:

```rust,ignore
use claude_codes::auth::{auth_status, LoginFlow, LoginMode};

let mut flow = LoginFlow::start(LoginMode::SetupToken)?;
let url = flow.auth_url(Duration::from_secs(30))?;   // show to the user
// … user authorizes in a browser, brings back a code …
let outcome = flow.submit_code_and_wait(&code, Duration::from_secs(90))?;
// outcome.token = Some("sk-ant-oat01-…") — recovered via screen text,
// OSC 52 clipboard escapes, or the credentials-file watch.
```

Rejected codes keep the flow alive: call `retry_new_url()` for a fresh
authorize URL (the CLI rotates the PKCE challenge; the old code is dead).
Every failure self-describes — timeouts and child exits carry a channel
line naming what each detection source saw. `auth_status()` types
`claude auth status --json` (email, org, plan). Enable with
`features = ["auth"]`.

## License

Apache-2.0. See [LICENSE](../LICENSE).
