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

/// The SHA-256 of the `codex-<triple>.tar.gz` release asset for a target triple.
pub(crate) fn codex_sha256(platform: &str) -> Option<String> {
    match platform {
        "x86_64-unknown-linux-musl" => Some(String::from(
            "a0bf572d2f93f056c6712a6da348e96078dc84f3885fd7be67807ef389cb3978",
        )),
        "aarch64-unknown-linux-musl" => Some(String::from(
            "94eb707605ec2646b2e05b7edf7c4e7b3ccf02b16237c34ae9beb8cfb7f810a9",
        )),
        "x86_64-apple-darwin" => Some(String::from(
            "db0c38713c36199b12e7c8d1629386c35a4ce8f99e1c9ce72f1725a74df67b57",
        )),
        "aarch64-apple-darwin" => Some(String::from(
            "c48824462fc57681eafb0925d97b11b589010475613904f570b42fea53e240d5",
        )),
        "x86_64-pc-windows-msvc" => Some(String::from(
            "dafeb48b5a853b2898453a6b5bdae0fa0646bd5d5b73c5dfe74c97d0af4a4a0d",
        )),
        "aarch64-pc-windows-msvc" => Some(String::from(
            "23cfbde45609292c325029a376340df30191298d7b6c5b26f607f481573ac768",
        )),
        _ => None,
    }
}
