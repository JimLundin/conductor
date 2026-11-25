use crate::prelude::*;

pub const VERSION: &str = "0.166.0";

pub const VERIFIED: &str = "every package matches GitHub's published digest and codex-package_SHA256SUMS; the Linux cosign bundles verify";

#[must_use]
pub fn sha256(platform: &str) -> Option<String> {
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
