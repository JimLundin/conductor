# Codex Linux package verification

On 2026-11-21, cosign v2.6.1 verified the release bundles for
`codex-package-x86_64-unknown-linux-musl.tar.gz` and
`codex-package-aarch64-unknown-linux-musl.tar.gz` from Codex
`rust-v0.166.0`. Both archives matched the SHA-256 values in
`crates/codex/src/pin/pins.rs`.

The expected certificate identity was
`https://github.com/openai/codex/.github/workflows/rust-release.yml@refs/tags/rust-v0.166.0`
and the expected OIDC issuer was
`https://token.actions.githubusercontent.com`. Verification used
`cosign verify-blob --bundle <package>.sigstore --certificate-identity <identity> --certificate-oidc-issuer <issuer> <package>`
for each package.
