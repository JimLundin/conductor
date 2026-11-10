#[derive(Debug, Clone)]
struct Listed {
    a: u32,
}

#[derive(Debug, serde::Serialize, Default)]
struct Unlisted {
    a: u32,
}

#[allow(dead_code)]
#[cfg(feature = "web")]
fn gated() {}
