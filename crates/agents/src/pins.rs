//! The pinned CLIs, written by `tools/pin` with `pins.toml`. Edit neither by
//! hand: run `tools/pin` again.

pub(crate) const CLAUDE_VERSION: &str = "2.1.316";

/// The SHA-256 of the `claude` binary for a platform named as in Claude's manifest.
pub(crate) fn claude_sha256(platform: &str) -> Option<String> {
    match platform {
        "darwin-arm64" => Some(String::from(
            "64c327e92b2c5e0f78c0a980c5a518121e1f0d140ca771ca5dd4488d1a2dcb3f",
        )),
        "darwin-x64" => Some(String::from(
            "203d22fbafdab9f6b0cda962ec573e32ab970894c63b1dfb0409378f28b9c344",
        )),
        "linux-arm64" => Some(String::from(
            "55cdb8e162d190432696ea54010e0fca2e18d06fa7a7127fd563b9ffd4f22624",
        )),
        "linux-x64" => Some(String::from(
            "fd13c70000fec3f2116dfbcbb6121210b4a29328bf0e46210a59dd594a67d824",
        )),
        "linux-arm64-musl" => Some(String::from(
            "d774cb39055cff62af476af32702f1fc4f888a7e395a37434a95d6f5a98818a0",
        )),
        "linux-x64-musl" => Some(String::from(
            "8bc3d015479b8b87dfde3f69f9bdb96c658758a8eb31892fd8fbece8d8a08bcf",
        )),
        "win32-x64" => Some(String::from(
            "e896a9d8da519353a49bf69ca4a695ba7e09067752687c675b3969041e632d52",
        )),
        "win32-arm64" => Some(String::from(
            "a44168f2360bf21709855321b25f1eeb021de8a8d4be5d25a8b397dcbf9043f8",
        )),
        _ => None,
    }
}

pub(crate) const CODEX_VERSION: &str = "0.166.0";

/// The SHA-256 of the `codex-package-<triple>.tar.gz` release asset for a target triple.
pub(crate) fn codex_sha256(platform: &str) -> Option<String> {
    match platform {
        "x86_64-unknown-linux-musl" => Some(String::from(
            "1d44b28bc4ac9937d7d3532264baa68a507ce364956a0d4ebb856f331cdee254",
        )),
        "aarch64-unknown-linux-musl" => Some(String::from(
            "7821b77dda2e6283fb1f63f3165177a271da8833e857cf673c6ca1c9db3bfcc6",
        )),
        "x86_64-apple-darwin" => Some(String::from(
            "1cb65690230be5982c654ba2655e3a3b594dce9c55b780e78bfdfeff829d7ad9",
        )),
        "aarch64-apple-darwin" => Some(String::from(
            "55a440cb579f970abe7c79ab181e54b0ea4c419be99f333bbd868eebac9c21f0",
        )),
        "x86_64-pc-windows-msvc" => Some(String::from(
            "d559bb88e74d471265857b93b7f19bc250dae527673b9191b33534478093f6bc",
        )),
        "aarch64-pc-windows-msvc" => Some(String::from(
            "9383152a72f97bb5dd148493c8214e4fed38d805aa2c3297cb3739f7f130becf",
        )),
        _ => None,
    }
}
