# Vendored crates

These crates are copied into this repository and maintained here as our own code.
We do not track or contribute to upstream.

| Crate | Version taken | Source |
|---|---|---|
| `claude-codes` | 2.1.323 | crates.io package of `meawoppl/rust-code-agent-sdks`, upstream main at `2d18526` on 2026-11-10 |
| `codex-codes` | 0.167.4 | same repository and date |

Both are licensed Apache-2.0 (`LICENSE-APACHE-rust-code-agent-sdks`). Files we change
keep the license; the repository `NOTICE` records that they were modified.

`docs/claude-codes-gaps.md` lists what we changed in `claude-codes` after taking it.

## Later: into the subset

These crates are outside the Conductor Rust subset (`docs/subset.md`) for
now. When they are rewritten into it, each moves out of `vendor/` into its
own crate under `crates/`, and this directory goes away.
