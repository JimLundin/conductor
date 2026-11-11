//! The weighted score: every token `measure` counts costs the weight of the
//! innermost construct that contains it.

use std::collections::BTreeMap;
use std::ops::{AddAssign, Range};

use crate::context::{Context, Place, TypeClass};
use crate::weights::{Tier, TierWeights};

/// One construct occurrence, with the id and tier the score charges it.
pub struct Judged {
    pub id: String,
    pub tier: Tier,
    pub extent: Range<usize>,
}

/// Tokens per tier, indexed by `Tier::index`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TierTokens(pub [usize; 6]);

impl TierTokens {
    pub fn tokens(&self) -> usize {
        self.0.iter().sum()
    }

    pub fn of(&self, tier: Tier) -> usize {
        self.0[tier.index()]
    }

    pub fn score_of(&self, tier: Tier, weights: &TierWeights) -> f64 {
        self.of(tier) as f64 * weights.of(tier)
    }

    pub fn score(&self, weights: &TierWeights) -> f64 {
        Tier::ALL.iter().map(|t| self.score_of(*t, weights)).sum()
    }
}

impl AddAssign for TierTokens {
    fn add_assign(&mut self, other: TierTokens) {
        for (mine, theirs) in self.0.iter_mut().zip(other.0) {
            *mine += theirs;
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Count {
    pub count: usize,
    pub tokens: usize,
}

pub struct FnTally {
    pub name: String,
    pub line: usize,
    pub tiers: TierTokens,
    /// Tokens of primitive types in the fn's signature.
    pub signature_primitives: usize,
}

#[derive(Default)]
pub struct FileTally {
    pub tiers: TierTokens,
    /// Occurrences and tokens per id and the tier it was charged at.
    pub constructs: BTreeMap<(String, Tier), Count>,
    pub fns: Vec<FnTally>,
    /// Tokens of field and signature types of our own items, per
    /// `TypeClass::index`.
    pub typing: [usize; 4],
}

/// The id of tokens that no construct contains.
pub const NO_CONSTRUCT: &str = "(no construct)";

/// Attributes each token at `starts` to the innermost construct containing
/// it. A construct inside a folded node owns no tokens, unless it is a
/// violation: the folded node's own construct takes them.
pub fn tally(src_len: usize, starts: &[usize], judged: &[Judged], context: &Context) -> FileTally {
    let folded = |j: &Judged| {
        j.tier != Tier::Outside
            && context
                .folded
                .iter()
                .any(|f| f.start <= j.extent.start && j.extent.end <= f.end && j.extent != *f)
    };
    let painted: Vec<Range<usize>> = judged
        .iter()
        .map(|j| match folded(j) {
            true => 0..0,
            false => j.extent.clone(),
        })
        .collect();
    let owners = innermost(src_len, &painted);
    let fn_extents: Vec<Range<usize>> = context.fns.iter().map(|f| f.extent.clone()).collect();
    let fn_owners = innermost(src_len, &fn_extents);
    let typed_extents: Vec<Range<usize>> = context.typed.iter().map(|t| t.extent.clone()).collect();
    let typed_owners = innermost(src_len, &typed_extents);

    let mut out = FileTally::default();
    let mut fn_tiers = vec![TierTokens::default(); context.fns.len()];
    let mut fn_primitives = vec![0; context.fns.len()];
    for j in judged {
        out.constructs
            .entry((j.id.clone(), j.tier))
            .or_default()
            .count += 1;
    }
    for &start in starts {
        let (id, tier) = match owners.get(start).copied().flatten() {
            Some(i) => (judged[i].id.as_str(), judged[i].tier),
            None => (NO_CONSTRUCT, Tier::Neutral),
        };
        out.constructs
            .entry((id.to_string(), tier))
            .or_default()
            .tokens += 1;
        out.tiers.0[tier.index()] += 1;
        let in_fn = fn_owners.get(start).copied().flatten();
        if let Some(f) = in_fn {
            fn_tiers[f].0[tier.index()] += 1;
        }
        if let Some(t) = typed_owners.get(start).copied().flatten() {
            let typed = &context.typed[t];
            out.typing[typed.class.index()] += 1;
            if let (TypeClass::Primitive, Place::Signature, Some(f)) =
                (typed.class, typed.place, in_fn)
            {
                fn_primitives[f] += 1;
            }
        }
    }
    out.fns = context
        .fns
        .iter()
        .zip(fn_tiers)
        .zip(fn_primitives)
        .map(|((f, tiers), signature_primitives)| FnTally {
            name: f.name.clone(),
            line: f.line,
            tiers,
            signature_primitives,
        })
        .collect();
    out
}

/// For each byte, the index of the smallest extent containing it. Among equal
/// extents the later one wins, since the classifier emits a node before its
/// children.
fn innermost(len: usize, extents: &[Range<usize>]) -> Vec<Option<usize>> {
    let mut order: Vec<usize> = (0..extents.len()).collect();
    order.sort_by_key(|&i| (std::cmp::Reverse(extents[i].len()), i));
    let mut owners = vec![None; len];
    for i in order {
        let range = extents[i].start.min(len)..extents[i].end.min(len);
        for owner in &mut owners[range] {
            *owner = Some(i);
        }
    }
    owners
}

/// A scored file and the crate it belongs to.
pub struct ScoredFile {
    pub path: String,
    pub krate: String,
    pub tally: FileTally,
    pub violations: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Crate,
    File,
    Fn,
    Construct,
}

impl Level {
    pub fn parse(name: &str) -> Option<Level> {
        match name {
            "crate" => Some(Level::Crate),
            "file" => Some(Level::File),
            "fn" => Some(Level::Fn),
            "construct" => Some(Level::Construct),
            _ => None,
        }
    }

    fn column(self) -> &'static str {
        match self {
            Level::Crate => "crate",
            Level::File => "file",
            Level::Fn => "fn",
            Level::Construct => "construct",
        }
    }
}

pub struct Row {
    pub name: String,
    /// Occurrences, for a construct row.
    pub count: Option<usize>,
    /// The tier a construct row was charged at.
    pub tier: Option<Tier>,
    pub tiers: TierTokens,
}

impl Row {
    pub fn score(&self, w: &TierWeights) -> f64 {
        self.tiers.score(w)
    }

    pub fn per_token(&self, w: &TierWeights) -> f64 {
        match self.tiers.tokens() {
            0 => 0.0,
            n => self.score(w) / n as f64,
        }
    }

    pub fn boundary(&self, w: &TierWeights) -> f64 {
        self.tiers.score_of(Tier::Boundary, w)
    }

    /// The boundary score as a share of the score, in percent.
    pub fn boundary_share(&self, w: &TierWeights) -> f64 {
        match self.score(w) {
            s if s > 0.0 => 100.0 * self.boundary(w) / s,
            _ => 0.0,
        }
    }
}

pub fn total(files: &[ScoredFile]) -> Row {
    let mut tiers = TierTokens::default();
    for f in files {
        tiers += f.tally.tiers;
    }
    Row {
        name: "total".to_string(),
        count: None,
        tier: None,
        tiers,
    }
}

/// Rows at one level: crates and files by name, functions and constructs by
/// score, highest first.
pub fn rows(files: &[ScoredFile], level: Level, w: &TierWeights) -> Vec<Row> {
    let mut rows: Vec<Row> = match level {
        Level::Crate => {
            let mut crates: BTreeMap<&str, TierTokens> = BTreeMap::new();
            for f in files {
                *crates.entry(&f.krate).or_default() += f.tally.tiers;
            }
            crates
                .into_iter()
                .map(|(name, tiers)| plain(name.to_string(), tiers))
                .collect()
        }
        Level::File => files
            .iter()
            .map(|f| plain(f.path.clone(), f.tally.tiers))
            .collect(),
        Level::Fn => files
            .iter()
            .flat_map(|f| {
                f.tally
                    .fns
                    .iter()
                    .map(|n| plain(format!("{}:{} {}", f.path, n.line, n.name), n.tiers))
            })
            .collect(),
        Level::Construct => {
            let mut all: BTreeMap<(String, Tier), Count> = BTreeMap::new();
            for f in files {
                for (key, c) in &f.tally.constructs {
                    let entry = all.entry(key.clone()).or_default();
                    entry.count += c.count;
                    entry.tokens += c.tokens;
                }
            }
            all.into_iter()
                .map(|((id, tier), c)| {
                    let mut tiers = TierTokens::default();
                    tiers.0[tier.index()] = c.tokens;
                    Row {
                        name: id,
                        count: Some(c.count),
                        tier: Some(tier),
                        tiers,
                    }
                })
                .collect()
        }
    };
    if matches!(level, Level::Fn | Level::Construct) {
        rows.sort_by(|a, b| b.score(w).total_cmp(&a.score(w)).then(a.name.cmp(&b.name)));
    }
    rows
}

fn plain(name: String, tiers: TierTokens) -> Row {
    Row {
        name,
        count: None,
        tier: None,
        tiers,
    }
}

pub fn text(rows: &[Row], total: &Row, level: Level, w: &TierWeights) -> String {
    let construct = level == Level::Construct;
    let mut out = format!(
        "{:>10} {:>8} {:>9} {:>9} {:>6}{} {}\n",
        "score",
        "tokens",
        "per token",
        "boundary",
        "share",
        match construct {
            true => format!(" {:>6} {:<10}", "count", "tier"),
            false => String::new(),
        },
        level.column()
    );
    for row in rows.iter().chain(std::iter::once(total)) {
        let extra = match (row.count, row.tier) {
            (Some(count), Some(tier)) => format!(" {count:>6} {:<10}", tier.name()),
            _ if construct => format!(" {:>6} {:<10}", "", ""),
            _ => String::new(),
        };
        out.push_str(&format!(
            "{:>10.1} {:>8} {:>9.3} {:>9.1} {:>5.1}%{extra} {}\n",
            row.score(w),
            row.tiers.tokens(),
            row.per_token(w),
            row.boundary(w),
            row.boundary_share(w),
            row.name
        ));
    }
    out
}

pub fn json(rows: &[Row], total: &Row, level: Level, w: &TierWeights) -> serde_json::Value {
    let one = |row: &Row| {
        let mut value = serde_json::json!({
            "name": row.name,
            "score": row.score(w),
            "tokens": row.tiers.tokens(),
            "per_token": row.per_token(w),
            "boundary": row.boundary(w),
            "boundary_share": row.boundary_share(w),
        });
        if let (Some(count), Some(tier)) = (row.count, row.tier) {
            value["count"] = count.into();
            value["tier"] = tier.name().into();
        }
        value
    };
    serde_json::json!({
        "by": level.column(),
        "rows": rows.iter().map(one).collect::<Vec<_>>(),
        "total": one(total),
    })
}
