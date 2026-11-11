//! Library traits and types the config names by path, such as the fallible
//! operator traits (proposal 0038) and the consuming readers (proposal
//! 0039), matched against paths as the code writes them.

use std::collections::BTreeMap;

/// A trait or type named by its path, as `std::ops::Add`.
pub struct Named {
    module: Vec<String>,
    name: String,
}

impl Named {
    pub fn parse(path: &str) -> Named {
        let mut segments = std_root(path.split("::").map(str::to_string).collect());
        let name = segments.pop().unwrap_or_default();
        Named {
            module: segments,
            name,
        }
    }

    /// The index of the segment that names this item in a path as written,
    /// where the segments before it agree with the item's module, as
    /// `ops::Add::add` for `std::ops::Add`.
    pub fn found_in(&self, segments: &[String]) -> Option<usize> {
        (0..segments.len()).find(|&i| {
            segments[i] == self.name && self.module.ends_with(&std_root(segments[..i].to_vec()))
        })
    }

    /// Whether a glob import of `prefix` brings this item into scope.
    pub fn globbed_by(&self, prefix: &[String]) -> bool {
        !prefix.is_empty() && self.module.ends_with(&std_root(prefix.to_vec()))
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

/// A fallible operator trait and the methods it gives (proposal 0038).
pub struct Operator {
    pub item: Named,
    pub methods: Vec<String>,
}

pub fn operators(config: &BTreeMap<String, Vec<String>>) -> Vec<Operator> {
    config
        .iter()
        .map(|(path, methods)| Operator {
            item: Named::parse(path),
            methods: methods.clone(),
        })
        .collect()
}

pub fn readers(config: &[String]) -> Vec<Named> {
    config.iter().map(|path| Named::parse(path)).collect()
}

/// `core::` and `alloc::` re-export into `std::`, so they read as `std::`.
fn std_root(mut segments: Vec<String>) -> Vec<String> {
    if let Some(first) = segments.first_mut()
        && ["core", "alloc"].contains(&first.as_str())
    {
        *first = "std".to_string();
    }
    segments
}

/// The default `fallible_operators`: the `std::ops` traits whose operation
/// can overflow or panic, with their methods. The bitwise traits and `Not`
/// are total and stay out.
pub fn default_fallible_operators() -> BTreeMap<String, Vec<String>> {
    ["Add", "Sub", "Mul", "Div", "Rem", "Neg", "Shl", "Shr"]
        .iter()
        .flat_map(|name| {
            let method = name.to_lowercase();
            let plain = (format!("std::ops::{name}"), vec![method.clone()]);
            let assign = (
                format!("std::ops::{name}Assign"),
                vec![format!("{method}_assign")],
            );
            match *name {
                "Neg" => vec![plain],
                _ => vec![plain, assign],
            }
        })
        .collect()
}

/// The default `consuming_readers`.
pub fn default_consuming_readers() -> Vec<String> {
    vec!["bytes::Buf".to_string()]
}
