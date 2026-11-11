//! The `[weights]` table of `subset.toml`: what a token costs, by the tier of
//! the construct it belongs to.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::context::Typing;

/// How the subset regards a construct. `Outside` is any violation, and
/// `Boundary` any construct whose shape an item we don't define fixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Encouraged,
    Neutral,
    Tolerated,
    Costly,
    Outside,
    Boundary,
}

impl Tier {
    pub const ALL: [Tier; 6] = [
        Tier::Encouraged,
        Tier::Neutral,
        Tier::Tolerated,
        Tier::Costly,
        Tier::Outside,
        Tier::Boundary,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Tier::Encouraged => "encouraged",
            Tier::Neutral => "neutral",
            Tier::Tolerated => "tolerated",
            Tier::Costly => "costly",
            Tier::Outside => "outside",
            Tier::Boundary => "boundary",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Tier::Encouraged => 0,
            Tier::Neutral => 1,
            Tier::Tolerated => 2,
            Tier::Costly => 3,
            Tier::Outside => 4,
            Tier::Boundary => 5,
        }
    }

    /// A tier the boundary context relieves when an argument, field value or
    /// type argument of an item we don't define stands in it.
    pub fn restricted(self) -> bool {
        matches!(self, Tier::Tolerated | Tier::Costly)
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TierWeights {
    pub encouraged: f64,
    pub neutral: f64,
    pub tolerated: f64,
    pub costly: f64,
    pub outside: f64,
    pub boundary: f64,
}

impl TierWeights {
    pub fn of(&self, tier: Tier) -> f64 {
        match tier {
            Tier::Encouraged => self.encouraged,
            Tier::Neutral => self.neutral,
            Tier::Tolerated => self.tolerated,
            Tier::Costly => self.costly,
            Tier::Outside => self.outside,
            Tier::Boundary => self.boundary,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Weights {
    pub tiers: TierWeights,
    /// The tier of each allowed id. An allowed id missing here is neutral.
    pub ids: BTreeMap<String, Tier>,
    /// Method names that make a call `expr.method_call.adapter`.
    #[serde(default = "default_adapters")]
    pub adapters: Vec<String>,
    /// Type patterns that are `type.primitive` in a field or signature.
    #[serde(default = "default_primitives")]
    pub primitives: Vec<String>,
    /// Type patterns that are `type.untyped` anywhere.
    #[serde(default = "default_untyped")]
    pub untyped: Vec<String>,
    /// Globs of the modules that mirror a protocol someone else defines.
    #[serde(default)]
    pub external_protocols: Vec<String>,
}

impl Weights {
    /// The tier of an allowed occurrence, before the boundary context.
    pub fn tier(&self, id: &str) -> Tier {
        self.ids.get(id).copied().unwrap_or(Tier::Neutral)
    }

    pub fn typing(&self) -> Result<Typing, String> {
        let parse = |patterns: &[String]| -> Result<Vec<syn::Type>, String> {
            patterns
                .iter()
                .map(|p| syn::parse_str(p).map_err(|e| format!("[weights] type pattern {p}: {e}")))
                .collect()
        };
        Ok(Typing {
            adapters: self.adapters.clone(),
            primitives: parse(&self.primitives)?,
            untyped: parse(&self.untyped)?,
        })
    }
}

fn strings(list: &[&str]) -> Vec<String> {
    list.iter().map(ToString::to_string).collect()
}

fn default_adapters() -> Vec<String> {
    strings(ITERATOR_ADAPTERS)
}

fn default_primitives() -> Vec<String> {
    strings(PRIMITIVES)
}

fn default_untyped() -> Vec<String> {
    strings(UNTYPED)
}

/// The standard library's primitive data: text, numbers and bytes.
pub const PRIMITIVES: &[&str] = &[
    "String", "str", "char", "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64",
    "u128", "usize", "f32", "f64", "Vec<u8>",
];

/// The standard library's ways to hold data without a type: a map keyed by
/// strings, and `dyn Any`.
pub const UNTYPED: &[&str] = &["HashMap<String, _>", "BTreeMap<String, _>", "dyn Any"];

/// The id of a field of a wire type in an external protocol module. Its
/// tokens always cost the boundary weight.
pub const WIRE_FIELD: &str = "field.wire";

/// Ids that exist only in the score: an allowed id refined by where it
/// stands, so it can take its own tier.
pub const SCORE_IDS: &[(&str, &str)] = &[
    (
        "expr.method_call.adapter",
        "method call whose name is in `adapters`",
    ),
    ("stmt.let.discard", "`let _ = value;`"),
    (
        "type.field",
        "type in a field of our own struct or enum variant",
    ),
    (
        "type.primitive",
        "type in `primitives`, as a field type or in a signature of our own",
    ),
    ("type.untyped", "type in `untyped`, anywhere"),
];

pub fn score_ids_table() -> String {
    let rows = SCORE_IDS
        .iter()
        .chain(&[(
            WIRE_FIELD,
            "field of a wire type in an external protocol module",
        )])
        .map(|(id, meaning)| format!("| `{id}` | {meaning} |\n"));
    std::iter::once("| Id | Meaning |\n|---|---|\n".to_string())
        .chain(rows)
        .collect()
}

/// The standard library's iterator methods.
pub const ITERATOR_ADAPTERS: &[&str] = &[
    "all",
    "any",
    "chain",
    "cloned",
    "collect",
    "copied",
    "count",
    "cycle",
    "enumerate",
    "filter",
    "filter_map",
    "find",
    "find_map",
    "flat_map",
    "flatten",
    "fold",
    "fuse",
    "inspect",
    "last",
    "map",
    "map_while",
    "max",
    "max_by",
    "max_by_key",
    "min",
    "min_by",
    "min_by_key",
    "nth",
    "partition",
    "peekable",
    "position",
    "product",
    "reduce",
    "rev",
    "rposition",
    "scan",
    "skip",
    "skip_while",
    "step_by",
    "sum",
    "take",
    "take_while",
    "try_fold",
    "unzip",
    "zip",
];
