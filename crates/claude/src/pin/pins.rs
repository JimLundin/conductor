//! The pinned claude, written by `tools/pin`. Run it again to change the pin.

use crate::prelude::*;

/// Verified: manifest.json.sig is a good signature by 31DDDE24DDFAB679F42D7BD2BAA929FF1A7ECACE; every binary matches the manifest
pub const VERSION: &str = "2.1.316";

/// The SHA-256 of the `claude` binary for a platform named as in Claude's manifest.
#[must_use]
pub fn sha256(platform: &str) -> Option<String> {
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
