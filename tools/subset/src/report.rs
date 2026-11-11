//! `report-html`: one static page with inline SVG charts and no script.
//! Each chart is drawn twice, for wide and narrow screens, and CSS shows
//! one, so its text stays legible at either width. Every value in a chart
//! is also in a table.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::context::TypeClass;
use crate::score::{self, Level, Row, ScoredFile, TierTokens};
use crate::weights::{Tier, TierWeights};

pub struct Group {
    pub title: String,
    /// One sentence on what the group is, as HTML.
    pub note: String,
    pub comparison: bool,
    pub files: Vec<ScoredFile>,
}

/// The score of each crate at one commit.
pub struct Snapshot {
    pub sha: String,
    pub date: String,
    pub crates: BTreeMap<String, f64>,
}

pub struct Page<'a> {
    pub stamp: String,
    pub groups: &'a [Group],
    /// The snapshots, or why there are none.
    pub history: Result<Vec<Snapshot>, String>,
    pub weights: &'a TierWeights,
}

const WIDE: f64 = 960.0;
const NARROW: f64 = 360.0;

pub fn render(page: &Page) -> String {
    let mut body = String::new();
    let _ = write!(
        body,
        "<header><h1>Subset score</h1><p class=\"sub\">{}</p>{}</header>",
        esc(&page.stamp),
        nav(page)
    );
    for (i, group) in page.groups.iter().enumerate() {
        body.push_str(&group_section(i, group, page.weights));
        if i == 0 {
            body.push_str(&history_section(&page.history));
        }
    }
    body.push_str(&method_section(page.weights));
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"color-scheme\" content=\"light dark\">\
         <title>Subset score</title><style>{CSS}</style></head>\
         <body><main class=\"viz-root\">{body}</main></body></html>\n"
    )
}

fn nav(page: &Page) -> String {
    let mut links: Vec<String> = page
        .groups
        .iter()
        .enumerate()
        .map(|(i, g)| format!("<a href=\"#group-{i}\">{}</a>", esc(&g.title)))
        .collect();
    links.insert(
        1.min(links.len()),
        "<a href=\"#history\">History</a>".to_string(),
    );
    links.push("<a href=\"#method\">How it's scored</a>".to_string());
    format!("<nav>{}</nav>", links.join(""))
}

fn group_section(i: usize, group: &Group, w: &TierWeights) -> String {
    let total = score::total(&group.files);
    let violations: usize = group.files.iter().map(|f| f.violations).sum();
    let crates = score::rows(&group.files, Level::Crate, w);
    let kind = match group.comparison {
        true => "<span class=\"badge\">Comparison</span> ",
        false => "",
    };
    let mut out = format!(
        "<section id=\"group-{i}\" class=\"group\"><h2>{kind}{}</h2><p class=\"note\">{}</p>",
        esc(&group.title),
        group.note
    );
    out.push_str(&tiles(&total, violations, w));
    out.push_str(&crate_breakdown(&crates, w));
    out.push_str(&treemap_section(i, &group.files, w));
    out.push_str(&typing_section(&group.files, w));
    out.push_str(&construct_table(&group.files, &total, group.comparison, w));
    out.push_str(&top_fns(&group.files, w));
    out.push_str("</section>");
    out
}

fn tiles(total: &Row, violations: usize, w: &TierWeights) -> String {
    let tile = |label: &str, value: String, hint: &str| {
        format!(
            "<div class=\"tile\"><div class=\"label\">{label}</div><div class=\"value\">{value}</div><div class=\"hint\">{hint}</div></div>"
        )
    };
    let in_lines = format!(
        "<div class=\"tile\" title=\"{APPROX_LINES}\"><div class=\"label\">Score in lines</div><div class=\"value\">{}</div><div class=\"hint\">approximate: score ÷ tokens per line</div></div>",
        approx_lines(total, w)
    );
    format!(
        "<div class=\"tiles\">{}{in_lines}{}{}{}{}</div>",
        tile("Score", whole(total.score(w)), "weighted tokens"),
        tile(
            "Tokens",
            whole(total.tiers.tokens() as f64),
            &format!("{} lines, comments excluded", whole(total.lines as f64))
        ),
        tile(
            "Score per token",
            format!("{:.3}", total.per_token(w)),
            "1.000 is all neutral"
        ),
        tile("Violations", whole(violations as f64), "what check reports"),
        tile(
            "Boundary share",
            format!("{:.1}%", total.boundary_share(w)),
            "of the score is library glue"
        ),
    )
}

fn legend_tiers() -> String {
    let items: String = Tier::ALL
        .iter()
        .map(|t| {
            format!(
                "<span class=\"key\"><span class=\"swatch\" style=\"background:var(--tier-{0})\"></span>{0}</span>",
                t.name()
            )
        })
        .collect();
    format!("<div class=\"legend\" aria-label=\"Tiers\">{items}</div>")
}

fn crate_breakdown(crates: &[Row], w: &TierWeights) -> String {
    let names: Vec<&str> = crates.iter().map(|r| r.name.as_str()).collect();
    let prefix = common_prefix(&names);
    let max = crates.iter().map(|r| r.score(w)).fold(0.0, f64::max);
    let chart = |width: f64, label_w: f64, value_w: f64| {
        let row_h = 30.0;
        let bar_h = 16.0;
        let plot = width - label_w - value_w;
        let height = row_h * crates.len() as f64 + 4.0;
        let mut svg = svg_open(width, height, "Score per crate, split by tier");
        for (r, row) in crates.iter().enumerate() {
            let y = 2.0 + r as f64 * row_h;
            let _ = write!(
                svg,
                "<text class=\"axis-label\" x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"end\">{}</text>",
                label_w - 8.0,
                y + row_h / 2.0 + 4.0,
                esc(&short(
                    row.name.strip_prefix(prefix).unwrap_or(&row.name),
                    label_w
                ))
            );
            let segments: Vec<(Tier, f64)> = Tier::ALL
                .iter()
                .map(|t| (*t, row.tiers.score_of(*t, w)))
                .filter(|(_, s)| *s > 0.0)
                .collect();
            let bar_y = y + (row_h - bar_h) / 2.0;
            svg.push_str(&stacked_bar(
                label_w,
                bar_y,
                bar_h,
                plot / max.max(1.0),
                &segments,
                |t, s| {
                    format!(
                        "{}: {} {} of {} ({:.1}%)\n{}, score {}",
                        row.name,
                        whole(s),
                        t.name(),
                        whole(row.score(w)),
                        100.0 * s / row.score(w),
                        volume(row.tiers.tokens(), row.lines),
                        approx_lines(row, w)
                    )
                },
                |t| format!("var(--tier-{})", t.name()),
            ));
            let end = label_w + row.score(w) / max.max(1.0) * plot;
            let label = match value_w > 100.0 {
                true => format!("{} · {}", whole(row.score(w)), approx_lines(row, w)),
                false => whole(row.score(w)),
            };
            let _ = write!(
                svg,
                "<text class=\"value-label\" x=\"{:.1}\" y=\"{:.1}\">{}<title>{APPROX_LINES}</title></text>",
                end + 6.0,
                y + row_h / 2.0 + 4.0,
                label
            );
        }
        let _ = write!(
            svg,
            "<line class=\"baseline\" x1=\"{label_w}\" x2=\"{label_w}\" y1=\"0\" y2=\"{height}\"/></svg>"
        );
        svg
    };
    let table_rows: String = crates
        .iter()
        .map(|r| {
            let cells: String = Tier::ALL
                .iter()
                .map(|t| format!("<td>{}</td>", whole(r.tiers.score_of(*t, w))))
                .collect();
            format!(
                "<tr><th scope=\"row\">{}</th><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{:.3}</td>{cells}<td>{:.1}%</td></tr>",
                esc(&r.name),
                whole(r.score(w)),
                approx_lines(r, w),
                whole(r.tiers.tokens() as f64),
                whole(r.lines as f64),
                r.per_token(w),
                r.boundary_share(w)
            )
        })
        .collect();
    let tier_heads: String = Tier::ALL
        .iter()
        .map(|t| format!("<th>{}</th>", t.name()))
        .collect();
    format!(
        "<h3>Score per crate, by tier</h3>{}{}<details><summary>Table</summary><div class=\"scroll\"><table><thead><tr><th>Crate</th><th>Score</th><th title=\"{APPROX_LINES}\">Score in lines</th><th>Tokens</th><th>Lines</th><th>Per token</th>{tier_heads}<th>Boundary share</th></tr></thead><tbody>{table_rows}</tbody></table></div></details>",
        legend_tiers(),
        two_widths(chart(WIDE, 220.0, 170.0), chart(NARROW, 110.0, 64.0)),
    )
}

/// What "score in lines" means, for its tooltips.
const APPROX_LINES: &str = "Approximate. The weighted score converted to lines at this scope's own tokens per line: score ÷ (tokens ÷ lines). Tokens per line vary, so read it as a sense of scale, not a count.";

/// "55,266 tokens · 8,740 lines".
fn volume(tokens: usize, lines: usize) -> String {
    format!(
        "{} tokens · {} lines",
        whole(tokens as f64),
        whole(lines as f64)
    )
}

/// The score as approximate lines, "≈ 7,452 lines".
fn approx_lines(row: &Row, w: &TierWeights) -> String {
    format!("≈ {} lines", whole(row.approx_lines(w)))
}

/// A horizontal bar of `segments` laid end to end from `x`, with a 2px
/// surface gap between them and a rounded data end.
fn stacked_bar<T: Copy>(
    x: f64,
    y: f64,
    h: f64,
    scale: f64,
    segments: &[(T, f64)],
    title: impl Fn(T, f64) -> String,
    fill: impl Fn(T) -> String,
) -> String {
    let mut out = String::new();
    let mut at = x;
    let last = segments.len().saturating_sub(1);
    for (i, (key, value)) in segments.iter().enumerate() {
        let width = value * scale;
        let gap = match i == last {
            true => 0.0,
            false => 2.0,
        };
        let drawn = (width - gap).max(0.5);
        let path = match i == last {
            true => end_rounded(at, y, drawn, h, 4.0_f64.min(drawn)),
            false => format!("M{at:.2},{y:.2}h{drawn:.2}v{h:.2}h{:.2}z", -drawn),
        };
        let _ = write!(
            out,
            "<path d=\"{path}\" fill=\"{}\"><title>{}</title></path>",
            fill(*key),
            esc(&title(*key, *value))
        );
        at += width;
    }
    out
}

/// A rectangle with its right corners rounded by `r`.
fn end_rounded(x: f64, y: f64, w: f64, h: f64, r: f64) -> String {
    format!(
        "M{x:.2},{y:.2}h{:.2}a{r:.2},{r:.2} 0 0 1 {r:.2},{r:.2}v{:.2}a{r:.2},{r:.2} 0 0 1 {:.2},{r:.2}h{:.2}z",
        w - r,
        h - 2.0 * r,
        -r,
        -(w - r)
    )
}

/// Score per token, binned onto the seven steps of the sequential ramp.
const BINS: [f64; 6] = [0.8, 0.95, 1.1, 1.3, 1.6, 2.0];

fn bin(per_token: f64) -> usize {
    BINS.iter().filter(|edge| per_token >= **edge).count()
}

fn treemap_section(group: usize, files: &[ScoredFile], w: &TierWeights) -> String {
    let mut sized: Vec<(&ScoredFile, f64)> = files
        .iter()
        .map(|f| (f, f.tally.tiers.score(w)))
        .filter(|(_, s)| *s > 0.0)
        .collect();
    sized.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.path.cmp(&b.0.path)));
    let crate_names: Vec<&str> = sized.iter().map(|(f, _)| f.krate.as_str()).collect();
    let prefix = common_prefix(&crate_names);
    let mut crates: Vec<Block> = Vec::new();
    for (file, score) in &sized {
        match crates.iter_mut().find(|b| b.krate == file.krate) {
            Some(block) => {
                block.score += score;
                block.files.push((file, *score));
            }
            None => crates.push(Block {
                krate: &file.krate,
                score: *score,
                files: vec![(file, *score)],
            }),
        }
    }
    crates.sort_by(|a, b| b.score.total_cmp(&a.score));
    let chart = |width: f64, height: f64| {
        let areas: Vec<f64> = crates.iter().map(|b| b.score).collect();
        let bounds = Rect {
            x: 0.0,
            y: 0.0,
            w: width,
            h: height,
        };
        let mut svg = svg_open(
            width,
            height,
            "Files grouped by crate, sized by score, coloured by score per token",
        );
        for (block, outer) in crates.iter().zip(squarify(&areas, bounds)) {
            let (krate, crate_score, members) = (block.krate, &block.score, &block.files);
            let header = match outer.h >= 60.0 && outer.w >= 60.0 {
                true => 18.0,
                false => 0.0,
            };
            if header > 0.0 {
                let name = krate.strip_prefix(prefix).unwrap_or(krate);
                let full = format!("{name} · {}", whole(*crate_score));
                let label = match short(&full, outer.w) == full {
                    true => full,
                    false => name.to_string(),
                };
                let _ = write!(
                    svg,
                    "<text class=\"crate-label\" x=\"{:.1}\" y=\"{:.1}\">{}</text>",
                    outer.x + 3.0,
                    outer.y + 13.0,
                    esc(&short(&label, outer.w))
                );
            }
            let inside = Rect {
                x: outer.x + 2.0,
                y: outer.y + header,
                w: (outer.w - 4.0).max(0.0),
                h: (outer.h - header - 4.0).max(0.0),
            };
            let file_areas: Vec<f64> = members.iter().map(|(_, s)| *s).collect();
            for ((file, score), rect) in members.iter().zip(squarify(&file_areas, inside)) {
                svg.push_str(&file_cell(file, &cell_name(file, members), *score, rect, w));
            }
        }
        svg.push_str("</svg>");
        svg
    };
    let steps: String = (0..=BINS.len())
        .map(|b| {
            let range = match b {
                0 => format!("&lt; {:.2}", BINS[0]),
                b if b == BINS.len() => format!("≥ {:.2}", BINS[b - 1]),
                b => format!("{:.2}–{:.2}", BINS[b - 1], BINS[b]),
            };
            format!(
                "<span class=\"key\"><span class=\"swatch\" style=\"background:var(--seq-{b})\"></span>{range}</span>"
            )
        })
        .collect();
    let table_rows: String = sized
        .iter()
        .map(|(f, s)| {
            let row = Row {
                name: f.path.clone(),
                count: None,
                tier: None,
                tiers: f.tally.tiers,
                lines: f.tally.lines,
            };
            format!(
                "<tr><th scope=\"row\">{}</th><td>{}</td><td class=\"opt\">{}</td><td class=\"opt\">{}</td><td>{:.3}</td><td class=\"opt\">{:.1}%</td><td>{}</td></tr>",
                breakable(&f.path),
                whole(*s),
                whole(f.tally.tiers.tokens() as f64),
                whole(f.tally.lines as f64),
                row.per_token(w),
                row.boundary_share(w),
                f.violations
            )
        })
        .collect();
    format!(
        "<h3 id=\"files-{group}\">Files</h3><p class=\"caption\">Each file's area is its score. Darker means more score per token: code that leans on compromises or breaks the subset.</p><div class=\"legend\" aria-label=\"Score per token\"><span class=\"legend-title\">Score per token</span>{steps}</div>{}<details><summary>Table</summary><div class=\"scroll\"><table><thead><tr><th>File</th><th>Score</th><th class=\"opt\">Tokens</th><th class=\"opt\">Lines</th><th>Per token</th><th class=\"opt\">Boundary share</th><th>Violations</th></tr></thead><tbody>{table_rows}</tbody></table></div></details>",
        two_widths(chart(WIDE, 560.0), chart(NARROW, 640.0))
    )
}

/// A crate's files in the treemap.
struct Block<'a> {
    krate: &'a str,
    score: f64,
    files: Vec<(&'a ScoredFile, f64)>,
}

/// One file's cell in the treemap, with its name and numbers where they fit.
/// A file's name in its crate's block: its base name, or its last two
/// components where another file in the block shares the base name.
fn cell_name(file: &ScoredFile, block: &[(&ScoredFile, f64)]) -> String {
    let base = |f: &ScoredFile| f.path.rsplit('/').next().unwrap_or("").to_string();
    let shared = block.iter().filter(|(o, _)| base(o) == base(file)).count() > 1;
    match shared {
        true => {
            let mut tail: Vec<&str> = file.path.rsplit('/').take(2).collect();
            tail.reverse();
            tail.join("/")
        }
        false => base(file),
    }
}

fn file_cell(file: &ScoredFile, name: &str, score: f64, rect: Rect, w: &TierWeights) -> String {
    let tiers = &file.tally.tiers;
    let per = score / tiers.tokens().max(1) as f64;
    let b = bin(per);
    let boundary = 100.0 * tiers.score_of(Tier::Boundary, w) / score;
    let inner = Rect {
        x: rect.x + 1.0,
        y: rect.y + 1.0,
        w: (rect.w - 2.0).max(0.0),
        h: (rect.h - 2.0).max(0.0),
    };
    let mut out = format!(
        "<g><rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" rx=\"2\" fill=\"var(--seq-{b})\"/><title>{}\nscore {} over {}\n{:.3} per token, boundary {:.1}%\n{} violations</title>",
        inner.x,
        inner.y,
        inner.w,
        inner.h,
        esc(&file.path),
        whole(score),
        volume(tiers.tokens(), file.tally.lines),
        per,
        boundary,
        file.violations
    );
    let numbers = format!("{} · {:.2}", whole(score), per);
    let longest = name.chars().count().max(numbers.chars().count()) as f64;
    if inner.w >= longest * 6.8 + 12.0 && inner.h >= 34.0 {
        let _ = write!(
            out,
            "<text class=\"cell\" x=\"{:.1}\" y=\"{:.1}\" fill=\"var(--on-seq-{b})\">{}</text><text class=\"cell sub\" x=\"{:.1}\" y=\"{:.1}\" fill=\"var(--on-seq-{b})\">{numbers}</text>",
            inner.x + 6.0,
            inner.y + 15.0,
            esc(name),
            inner.x + 6.0,
            inner.y + 29.0,
        );
    }
    out.push_str("</g>");
    out
}

/// The directory prefix, ending in `/`, that all `names` share.
fn common_prefix<'a>(names: &[&'a str]) -> &'a str {
    let Some(first) = names.first() else {
        return "";
    };
    let mut end = first.rfind('/').map_or(0, |i| i + 1);
    for name in names {
        while !name.starts_with(&first[..end]) {
            end = first[..end.saturating_sub(1)]
                .rfind('/')
                .map_or(0, |i| i + 1);
        }
    }
    &first[..end]
}

#[derive(Debug, Clone, Copy)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

/// The squarified treemap layout of `areas`, sorted largest first, in
/// `bounds`. Each rectangle's area is proportional to its value.
fn squarify(areas: &[f64], bounds: Rect) -> Vec<Rect> {
    let total: f64 = areas.iter().sum();
    if total <= 0.0 {
        return Vec::new();
    }
    let scale = bounds.w * bounds.h / total;
    let scaled: Vec<f64> = areas.iter().map(|a| a * scale).collect();
    let mut out = Vec::new();
    let mut free = bounds;
    let mut start = 0;
    while start < scaled.len() {
        let side = free.w.min(free.h);
        let mut end = start + 1;
        while end < scaled.len()
            && worst(&scaled[start..=end], side) <= worst(&scaled[start..end], side)
        {
            end += 1;
        }
        let row = &scaled[start..end];
        let sum: f64 = row.iter().sum();
        let horizontal = free.w >= free.h;
        let thickness = sum / side;
        let mut along = 0.0;
        for a in row {
            let length = a / thickness;
            out.push(match horizontal {
                true => Rect {
                    x: free.x,
                    y: free.y + along,
                    w: thickness,
                    h: length,
                },
                false => Rect {
                    x: free.x + along,
                    y: free.y,
                    w: length,
                    h: thickness,
                },
            });
            along += length;
        }
        free = match horizontal {
            true => Rect {
                x: free.x + thickness,
                w: free.w - thickness,
                ..free
            },
            false => Rect {
                y: free.y + thickness,
                h: free.h - thickness,
                ..free
            },
        };
        start = end;
    }
    out
}

/// The worst aspect ratio of a row of `areas` laid along `side`.
fn worst(areas: &[f64], side: f64) -> f64 {
    let sum: f64 = areas.iter().sum();
    let max = areas.iter().copied().fold(0.0, f64::max);
    let min = areas.iter().copied().fold(f64::INFINITY, f64::min);
    let s2 = side * side;
    (s2 * max / (sum * sum)).max(sum * sum / (s2 * min))
}

fn typing_section(files: &[ScoredFile], w: &TierWeights) -> String {
    let mut crates: BTreeMap<&str, [usize; 4]> = BTreeMap::new();
    for f in files {
        let entry = crates.entry(&f.krate).or_default();
        for (sum, n) in entry.iter_mut().zip(f.tally.typing) {
            *sum += n;
        }
    }
    let rows: Vec<(&str, [usize; 4])> = crates
        .into_iter()
        .filter(|(_, t)| t.iter().sum::<usize>() > 0)
        .collect();
    if rows.is_empty() {
        return String::new();
    }
    let names: Vec<&str> = rows.iter().map(|(n, _)| *n).collect();
    let prefix = common_prefix(&names);
    let chart = |width: f64, label_w: f64| {
        let row_h = 30.0;
        let bar_h = 16.0;
        let value_w = 56.0;
        let plot = width - label_w - value_w;
        let height = row_h * rows.len() as f64 + 4.0;
        let mut svg = svg_open(
            width,
            height,
            "Typing of field and signature type tokens, per crate",
        );
        for (r, (name, typing)) in rows.iter().enumerate() {
            let y = 2.0 + r as f64 * row_h;
            let total = typing.iter().sum::<usize>() as f64;
            let _ = write!(
                svg,
                "<text class=\"axis-label\" x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"end\">{}</text>",
                label_w - 8.0,
                y + row_h / 2.0 + 4.0,
                esc(&short(name.strip_prefix(prefix).unwrap_or(name), label_w))
            );
            let segments: Vec<(TypeClass, f64)> = TypeClass::ALL
                .iter()
                .map(|c| (*c, typing[c.index()] as f64))
                .filter(|(_, n)| *n > 0.0)
                .collect();
            svg.push_str(&stacked_bar(
                label_w,
                y + (row_h - bar_h) / 2.0,
                bar_h,
                plot / total,
                &segments,
                |c, n| {
                    format!(
                        "{name}: {} {} tokens of {} ({:.1}%)",
                        n,
                        c.name(),
                        total,
                        100.0 * n / total
                    )
                },
                |c| format!("var(--type-{})", c.name()),
            ));
            let own = 100.0 * typing[TypeClass::Own.index()] as f64 / total;
            let _ = write!(
                svg,
                "<text class=\"value-label\" x=\"{:.1}\" y=\"{:.1}\">{own:.0}% own</text>",
                label_w + plot + 6.0,
                y + row_h / 2.0 + 4.0
            );
        }
        svg.push_str("</svg>");
        svg
    };
    let legend: String = TypeClass::ALL
        .iter()
        .map(|c| {
            format!(
                "<span class=\"key\"><span class=\"swatch\" style=\"background:var(--type-{0})\"></span>{0}</span>",
                c.name()
            )
        })
        .collect();
    let table_rows: String = rows
        .iter()
        .map(|(name, t)| {
            let total = t.iter().sum::<usize>().max(1) as f64;
            let cells: String = t
                .iter()
                .map(|n| format!("<td>{n} ({:.0}%)</td>", 100.0 * *n as f64 / total))
                .collect();
            format!("<tr><th scope=\"row\">{}</th>{cells}</tr>", esc(name))
        })
        .collect();
    let mut fns: Vec<(&ScoredFile, &score::FnTally)> = files
        .iter()
        .flat_map(|f| f.tally.fns.iter().map(move |n| (f, n)))
        .filter(|(_, n)| n.signature_primitives > 0)
        .collect();
    fns.sort_by(|a, b| {
        b.1.signature_primitives
            .cmp(&a.1.signature_primitives)
            .then(a.0.path.cmp(&b.0.path))
            .then(a.1.line.cmp(&b.1.line))
    });
    let fn_rows: String = fns
        .iter()
        .take(20)
        .enumerate()
        .map(|(i, (f, n))| {
            format!(
                "<tr><td>{}</td><th scope=\"row\"><code>{}</code></th><td class=\"path\">{}:{}</td><td>{}</td><td class=\"opt\">{}</td></tr>",
                i + 1,
                breakable(&n.name),
                breakable(&f.path),
                n.line,
                n.signature_primitives,
                whole(n.tiers.score(w))
            )
        })
        .collect();
    let class_heads: String = TypeClass::ALL
        .iter()
        .map(|c| format!("<th>{}</th>", c.name()))
        .collect();
    format!(
        "<h3>Typing</h3><p class=\"caption\">Tokens of the types in our own fields and signatures: our own named types, primitives (<code>String</code>, integers, <code>Vec&lt;u8&gt;</code> and the like), untyped data, and other types such as library types. A std container such as <code>Option</code> counts as what it holds.</p><div class=\"legend\" aria-label=\"Type classes\">{legend}</div>{}<details><summary>Table</summary><div class=\"scroll\"><table><thead><tr><th>Crate</th>{class_heads}</tr></thead><tbody>{table_rows}</tbody></table></div></details><h4>Newtype candidates: most primitive tokens in a signature</h4><div class=\"scroll\"><table><thead><tr><th>#</th><th class=\"l\">Function</th><th class=\"l\">Location</th><th>Primitive tokens</th><th class=\"opt\">Score</th></tr></thead><tbody>{fn_rows}</tbody></table></div>",
        two_widths(chart(WIDE, 220.0), chart(NARROW, 110.0))
    )
}

fn construct_table(files: &[ScoredFile], total: &Row, comparison: bool, w: &TierWeights) -> String {
    let rows = score::rows(files, Level::Construct, w);
    let whole_score = total.score(w).max(1.0);
    let max = rows.first().map_or(1.0, |r| r.score(w)).max(1.0);
    let html: Vec<String> = rows
        .iter()
        .map(|r| {
            let tier = r.tier.unwrap_or(Tier::Neutral).name();
            let share = 100.0 * r.score(w) / whole_score;
            let bar = (64.0 * r.score(w) / max).max(1.0);
            format!(
                "<tr><th scope=\"row\"><code>{id}</code></th><td class=\"tier\"><span class=\"swatch\" style=\"background:var(--tier-{tier})\"></span>{tier}</td><td class=\"opt\">{count}</td><td class=\"opt\">{tokens}</td><td class=\"opt\">{lines}</td><td class=\"opt\">{weight}</td><td>{score}</td><td class=\"share opt\"><svg width=\"64\" height=\"8\" aria-hidden=\"true\"><rect width=\"{bar:.1}\" height=\"8\" rx=\"2\" fill=\"var(--tier-{tier})\"/></svg> {share:.1}%</td></tr>",
                id = esc(&r.name),
                count = whole(r.count.unwrap_or(0) as f64),
                tokens = whole(r.tiers.tokens() as f64),
                lines = whole(r.lines as f64),
                weight = w.of(r.tier.unwrap_or(Tier::Neutral)),
                score = whole(r.score(w)),
            )
        })
        .collect();
    let table = |rows: &[String]| {
        format!(
            "<div class=\"scroll\"><table><thead><tr><th>Id</th><th class=\"l\">Tier</th><th class=\"opt\">Count</th><th class=\"opt\">Tokens</th><th class=\"opt\" title=\"Lines on which the id owns a token. A line holds several ids, so the rows overlap.\">Lines</th><th class=\"opt\">Weight</th><th>Score</th><th class=\"opt\">Share</th></tr></thead><tbody>{}</tbody></table></div>",
            rows.concat()
        )
    };
    let top = match comparison {
        true => String::new(),
        false => table(&html[..html.len().min(TOP_CONSTRUCTS)]),
    };
    format!(
        "<h3>Constructs</h3><p class=\"caption\">Each id at the tier it was charged, sorted by its contribution to the score. An id can appear twice: allowed in one place and a violation in another.</p>{top}<details><summary>All {} rows</summary>{}</details>",
        html.len(),
        table(&html)
    )
}

/// The construct rows shown before the full table.
const TOP_CONSTRUCTS: usize = 25;

fn top_fns(files: &[ScoredFile], w: &TierWeights) -> String {
    let fns: Vec<(&ScoredFile, &score::FnTally)> = files
        .iter()
        .flat_map(|f| f.tally.fns.iter().map(move |n| (f, n)))
        .collect();
    let per_token = |t: &TierTokens| t.score(w) / t.tokens().max(1) as f64;
    let mut by_score = fns.clone();
    by_score.sort_by(|a, b| b.1.tiers.score(w).total_cmp(&a.1.tiers.score(w)));
    let mut by_density: Vec<_> = fns
        .iter()
        .filter(|(_, n)| n.tiers.tokens() >= MIN_TOKENS)
        .copied()
        .collect();
    by_density.sort_by(|a, b| per_token(&b.1.tiers).total_cmp(&per_token(&a.1.tiers)));
    let table = |list: &[(&ScoredFile, &score::FnTally)]| {
        let rows: String = list
            .iter()
            .take(20)
            .enumerate()
            .map(|(i, (f, n))| {
                let row = Row {
                    name: n.name.clone(),
                    count: None,
                    tier: None,
                    tiers: n.tiers,
                    lines: n.lines,
                };
                format!(
                    "<tr><td>{}</td><th scope=\"row\"><code>{}</code></th><td class=\"path\">{}:{}</td><td>{}</td><td class=\"opt\">{}</td><td class=\"opt\">{}</td><td>{:.3}</td><td class=\"opt\">{:.1}%</td></tr>",
                    i + 1,
                    breakable(&n.name),
                    breakable(&f.path),
                    n.line,
                    whole(row.score(w)),
                    whole(row.tiers.tokens() as f64),
                    whole(row.lines as f64),
                    row.per_token(w),
                    row.boundary_share(w)
                )
            })
            .collect();
        format!(
            "<div class=\"scroll\"><table><thead><tr><th>#</th><th class=\"l\">Function</th><th class=\"l\">Location</th><th>Score</th><th class=\"opt\">Tokens</th><th class=\"opt\">Lines</th><th>Per token</th><th class=\"opt\">Boundary</th></tr></thead><tbody>{rows}</tbody></table></div>"
        )
    };
    format!(
        "<h3>Functions</h3><h4>Top 20 by score</h4>{}<h4>Top 20 by score per token</h4><p class=\"caption\">Functions of at least {MIN_TOKENS} tokens.</p>{}",
        table(&by_score),
        table(&by_density)
    )
}

/// The smallest fn ranked by score per token, so one-liners don't crowd
/// the list.
const MIN_TOKENS: usize = 30;

/// Categorical slots for crate lines, in fixed order.
const SERIES: usize = 8;

fn history_section(history: &Result<Vec<Snapshot>, String>) -> String {
    let snapshots = match history {
        Ok(s) if s.len() >= 2 => s,
        Ok(_) => {
            return "<section id=\"history\"><h2>History</h2><p class=\"note\">Fewer than two commits to compare.</p></section>".to_string();
        }
        Err(reason) => {
            return format!(
                "<section id=\"history\"><h2>History</h2><p class=\"note\">{}</p></section>",
                esc(reason)
            );
        }
    };
    let mut names: Vec<&str> = snapshots
        .iter()
        .flat_map(|s| s.crates.keys().map(String::as_str))
        .collect();
    names.sort();
    names.dedup();
    let slot = |name: &str| names.iter().position(|n| *n == name).unwrap_or(0) % SERIES + 1;
    let max = snapshots
        .iter()
        .flat_map(|s| s.crates.values().copied())
        .fold(0.0, f64::max)
        .max(1.0);
    let step = nice_step(max / 4.0);
    let top = (max / step).ceil() * step;
    let chart = |width: f64, height: f64, label_room: f64| {
        let left = 52.0;
        let right = label_room;
        let top_pad = 12.0;
        let bottom = 28.0;
        let plot_w = width - left - right;
        let plot_h = height - top_pad - bottom;
        let x = |i: usize| left + plot_w * i as f64 / (snapshots.len() - 1) as f64;
        let y = |v: f64| top_pad + plot_h * (1.0 - v / top);
        let mut svg = svg_open(width, height, "Score per crate across commits");
        let steps = (top / step).round() as usize;
        for k in 0..=steps {
            let v = step * k as f64;
            let _ = write!(
                svg,
                "<line class=\"grid\" x1=\"{left}\" x2=\"{:.1}\" y1=\"{:.1}\" y2=\"{:.1}\"/><text class=\"tick\" x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"end\">{}</text>",
                left + plot_w,
                y(v),
                y(v),
                left - 6.0,
                y(v) + 4.0,
                compact(v)
            );
        }
        let last = snapshots.len() - 1;
        let middle = snapshots.len() / 2;
        let distinct = |i: usize, j: usize| snapshots[i].date != snapshots[j].date;
        let ticks: Vec<usize> =
            match width > NARROW && distinct(middle, 0) && distinct(middle, last) {
                true => vec![0, middle, last],
                false => vec![0, last],
            };
        for i in &ticks {
            let anchor = match *i {
                0 => "start",
                i if i == last => "end",
                _ => "middle",
            };
            let _ = write!(
                svg,
                "<text class=\"tick\" x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"{anchor}\">{}</text>",
                x(*i),
                height - 8.0,
                esc(&snapshots[*i].date)
            );
        }
        for name in &names {
            let points: Vec<String> = snapshots
                .iter()
                .enumerate()
                .filter_map(|(i, s)| {
                    s.crates
                        .get(*name)
                        .map(|v| format!("{:.1},{:.1}", x(i), y(*v)))
                })
                .collect();
            let _ = write!(
                svg,
                "<polyline class=\"series\" points=\"{}\" stroke=\"var(--series-{})\"/>",
                points.join(" "),
                slot(name)
            );
        }
        let mut ends: Vec<(&str, f64)> = names
            .iter()
            .filter_map(|n| snapshots[last].crates.get(*n).map(|v| (*n, y(*v))))
            .collect();
        ends.sort_by(|a, b| a.1.total_cmp(&b.1));
        let mut placed: Vec<f64> = Vec::new();
        for (name, end_y) in &ends {
            let label_y = placed.last().map_or(*end_y, |prev| end_y.max(prev + 14.0));
            placed.push(label_y);
            let _ = write!(
                svg,
                "<circle cx=\"{:.1}\" cy=\"{end_y:.1}\" r=\"4\" fill=\"var(--series-{})\" class=\"ring\"/>",
                x(last),
                slot(name)
            );
            if (label_y - end_y).abs() > 0.5 {
                let _ = write!(
                    svg,
                    "<line class=\"leader\" x1=\"{:.1}\" y1=\"{end_y:.1}\" x2=\"{:.1}\" y2=\"{:.1}\"/>",
                    x(last) + 5.0,
                    x(last) + 12.0,
                    label_y - 4.0
                );
            }
            let _ = write!(
                svg,
                "<text class=\"axis-label\" x=\"{:.1}\" y=\"{:.1}\">{}</text>",
                x(last) + 14.0,
                label_y,
                esc(name.rsplit('/').next().unwrap_or(name))
            );
        }
        let band = plot_w / (snapshots.len() - 1) as f64;
        for (i, s) in snapshots.iter().enumerate() {
            let lines: Vec<String> = names
                .iter()
                .filter_map(|n| s.crates.get(*n).map(|v| format!("{n}: {}", whole(*v))))
                .collect();
            let _ = write!(
                svg,
                "<rect class=\"hit\" x=\"{:.1}\" y=\"{top_pad}\" width=\"{band:.1}\" height=\"{plot_h:.1}\"><title>{} {}\n{}</title></rect>",
                x(i) - band / 2.0,
                esc(&s.date),
                esc(&s.sha[..s.sha.len().min(8)]),
                esc(&lines.join("\n"))
            );
        }
        svg.push_str("</svg>");
        svg
    };
    let legend: String = names
        .iter()
        .map(|n| {
            format!(
                "<span class=\"key\"><span class=\"line-key\" style=\"background:var(--series-{})\"></span>{}</span>",
                slot(n),
                esc(n)
            )
        })
        .collect();
    let heads: String = names
        .iter()
        .map(|n| format!("<th>{}</th>", esc(n)))
        .collect();
    let rows: String = snapshots
        .iter()
        .rev()
        .map(|s| {
            let cells: String = names
                .iter()
                .map(|n| {
                    format!(
                        "<td>{}</td>",
                        s.crates.get(*n).map_or("–".to_string(), |v| whole(*v))
                    )
                })
                .collect();
            format!(
                "<tr><th scope=\"row\">{} <code>{}</code></th>{cells}</tr>",
                esc(&s.date),
                esc(&s.sha[..s.sha.len().min(8)])
            )
        })
        .collect();
    format!(
        "<section id=\"history\"><h2>History</h2><p class=\"note\">Score per crate at each of the last {} commits that changed the scored files, under today's <code>subset.toml</code>.</p><div class=\"legend\" aria-label=\"Crates\">{legend}</div>{}<details><summary>Table</summary><div class=\"scroll\"><table><thead><tr><th>Commit</th>{heads}</tr></thead><tbody>{rows}</tbody></table></div></details></section>",
        snapshots.len(),
        two_widths(chart(WIDE, 340.0, 90.0), chart(NARROW, 300.0, 74.0))
    )
}

fn method_section(w: &TierWeights) -> String {
    let rows: String = Tier::ALL
        .iter()
        .map(|t| {
            format!(
                "<tr><th scope=\"row\"><span class=\"swatch\" style=\"background:var(--tier-{0})\"></span>{0}</th><td>{1}</td></tr>",
                t.name(),
                w.of(*t)
            )
        })
        .collect();
    format!(
        "<section id=\"method\"><h2>How it's scored</h2><p>Every token costs the weight of the innermost construct that contains it. Comments cost nothing. A violation costs <em>outside</em> whatever its id, and a construct whose shape a library fixes costs <em>boundary</em>. The weights come from <code>[weights]</code> in <code>subset.toml</code>.</p><div class=\"scroll\"><table class=\"narrow\"><thead><tr><th>Tier</th><th>Weight</th></tr></thead><tbody>{rows}</tbody></table></div></section>"
    )
}

fn svg_open(width: f64, height: f64, label: &str) -> String {
    format!(
        "<svg viewBox=\"0 0 {width} {height}\" width=\"100%\" role=\"img\" aria-label=\"{}\">",
        esc(label)
    )
}

fn two_widths(wide: String, narrow: String) -> String {
    format!("<div class=\"chart wide\">{wide}</div><div class=\"chart narrow\">{narrow}</div>")
}

/// `name` cut to fit `room` pixels of 12px text, keeping its end.
fn short(name: &str, room: f64) -> String {
    let fits = ((room - 12.0) / 6.8) as usize;
    let chars: Vec<char> = name.chars().collect();
    match chars.len() > fits && fits > 1 {
        true => format!(
            "…{}",
            chars[chars.len() - (fits - 1)..].iter().collect::<String>()
        ),
        false => name.to_string(),
    }
}

/// The smallest of 1, 2, 2.5 and 5 times a power of ten that is at least `v`.
fn nice_step(v: f64) -> f64 {
    let magnitude = 10f64.powf(v.max(1.0).log10().floor());
    [1.0, 2.0, 2.5, 5.0, 10.0]
        .iter()
        .map(|m| m * magnitude)
        .find(|c| *c >= v)
        .unwrap_or(10.0 * magnitude)
}

/// A whole number with thousands separators.
fn whole(v: f64) -> String {
    let n = v.round() as i64;
    let digits = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    match n < 0 {
        true => format!("-{out}"),
        false => out,
    }
}

/// An axis tick: whole below 10,000, then `12K`.
fn compact(v: f64) -> String {
    match v >= 10_000.0 {
        true => format!("{}K", (v / 1000.0).round()),
        false => whole(v),
    }
}

/// `s` escaped, with break opportunities after `/`, `::` and `_`.
fn breakable(s: &str) -> String {
    esc(s)
        .replace('/', "/<wbr>")
        .replace("::", "::<wbr>")
        .replace('_', "_<wbr>")
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const CSS: &str = r#"
.viz-root {
  color-scheme: light;
  --surface-1: #fcfcfb; --page: #f9f9f7;
  --text-primary: #0b0b0b; --text-secondary: #52514e; --text-muted: #898781;
  --grid: #e1e0d9; --axis: #c3c2b7; --border: rgba(11,11,11,0.10);
  --tier-encouraged: #86b6ef; --tier-neutral: #5598e7; --tier-tolerated: #2a78d6;
  --tier-costly: #1c5cab; --tier-outside: #104281; --tier-boundary: #898781;
  --seq-0: #cde2fb; --seq-1: #9ec5f4; --seq-2: #6da7ec; --seq-3: #3987e5;
  --seq-4: #256abf; --seq-5: #184f95; --seq-6: #0d366b;
  --on-seq-0: #0b0b0b; --on-seq-1: #0b0b0b; --on-seq-2: #0b0b0b; --on-seq-3: #ffffff;
  --on-seq-4: #ffffff; --on-seq-5: #ffffff; --on-seq-6: #ffffff;
  --type-own: #2a78d6; --type-primitive: #eb6834; --type-untyped: #1baf7a; --type-other: #c3c2b7;
  --series-1: #2a78d6; --series-2: #eb6834; --series-3: #1baf7a; --series-4: #eda100;
  --series-5: #e87ba4; --series-6: #008300; --series-7: #6250d6; --series-8: #e34948;
}
@media (prefers-color-scheme: dark) {
  .viz-root {
    color-scheme: dark;
    --surface-1: #1a1a19; --page: #0d0d0d;
    --text-primary: #f0efec; --text-secondary: #c3c2b7; --text-muted: #898781;
    --grid: #2c2c2a; --axis: #383835; --border: rgba(255,255,255,0.10);
    --tier-encouraged: #184f95; --tier-neutral: #256abf; --tier-tolerated: #3987e5;
    --tier-costly: #6da7ec; --tier-outside: #9ec5f4; --tier-boundary: #898781;
    --seq-0: #0d366b; --seq-1: #184f95; --seq-2: #256abf; --seq-3: #3987e5;
    --seq-4: #6da7ec; --seq-5: #9ec5f4; --seq-6: #cde2fb;
    --on-seq-0: #ffffff; --on-seq-1: #ffffff; --on-seq-2: #ffffff; --on-seq-3: #ffffff;
    --on-seq-4: #0b0b0b; --on-seq-5: #0b0b0b; --on-seq-6: #0b0b0b;
    --type-own: #3987e5; --type-primitive: #d95926; --type-untyped: #199e70; --type-other: #52514e;
    --series-1: #3987e5; --series-2: #d95926; --series-3: #199e70; --series-4: #c98500;
    --series-5: #d55181; --series-6: #008300; --series-7: #9085e9; --series-8: #e66767;
  }
}
html { background: var(--page, #f9f9f7); }
@media (prefers-color-scheme: dark) { html { background: #0d0d0d; } }
body { margin: 0; font: 15px/1.5 system-ui, -apple-system, "Segoe UI", sans-serif; }
.viz-root { background: var(--page); color: var(--text-primary); max-width: 1000px; margin: 0 auto; padding: 24px 20px 64px; box-sizing: border-box; }
header h1 { font-size: 28px; margin: 0; }
.sub, .note, .caption { color: var(--text-secondary); margin: 4px 0 12px; }
.caption { font-size: 14px; }
nav { display: flex; flex-wrap: wrap; gap: 8px 16px; margin: 12px 0 8px; font-size: 14px; }
nav a, a { color: var(--text-primary); }
section { margin-top: 40px; }
h2 { font-size: 22px; margin: 0 0 4px; }
h3 { font-size: 17px; margin: 32px 0 4px; }
h4 { font-size: 15px; margin: 20px 0 8px; color: var(--text-secondary); font-weight: 600; }
.badge { font-size: 12px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.04em; border: 1px solid var(--axis); border-radius: 4px; padding: 1px 6px; vertical-align: 3px; color: var(--text-secondary); }
.tiles { display: grid; grid-template-columns: repeat(auto-fit, minmax(160px, 1fr)); gap: 12px; margin: 16px 0; }
.tile { background: var(--surface-1); border: 1px solid var(--border); border-radius: 8px; padding: 12px 14px; }
.tile .label { font-size: 13px; color: var(--text-secondary); }
.tile .value { font-size: 26px; font-weight: 600; margin: 2px 0; }
.tile .hint { font-size: 12px; color: var(--text-muted); }
.legend { display: flex; flex-wrap: wrap; gap: 6px 14px; font-size: 13px; color: var(--text-secondary); margin: 8px 0; align-items: center; }
.legend-title { color: var(--text-primary); font-weight: 600; }
.key { display: inline-flex; align-items: center; gap: 6px; }
.swatch { display: inline-block; width: 10px; height: 10px; border-radius: 2px; margin-right: 6px; vertical-align: -1px; }
.key .swatch { margin-right: 0; }
.line-key { display: inline-block; width: 14px; height: 2px; border-radius: 1px; }
.chart { background: var(--surface-1); border: 1px solid var(--border); border-radius: 8px; padding: 12px; margin: 8px 0; }
.chart svg { display: block; overflow: visible; }
.chart.narrow { display: none; }
@media (max-width: 700px) { .chart.wide { display: none; } .chart.narrow { display: block; } .viz-root { padding: 16px 12px 48px; } }
svg text { font-family: system-ui, -apple-system, "Segoe UI", sans-serif; }
.axis-label { font-size: 12px; fill: var(--text-secondary); }
.value-label { font-size: 12px; fill: var(--text-primary); font-variant-numeric: tabular-nums; }
.tick { font-size: 11px; fill: var(--text-muted); font-variant-numeric: tabular-nums; }
.crate-label { font-size: 12px; font-weight: 600; fill: var(--text-secondary); }
.cell { font-size: 12px; font-weight: 600; pointer-events: none; }
.cell.sub { font-weight: 400; font-size: 11px; }
.baseline { stroke: var(--axis); stroke-width: 1; }
.grid { stroke: var(--grid); stroke-width: 1; }
.series { fill: none; stroke-width: 2; stroke-linejoin: round; stroke-linecap: round; }
.ring { stroke: var(--surface-1); stroke-width: 2; }
.leader { stroke: var(--text-muted); stroke-width: 1; }
.hit { fill: transparent; }
.hit:hover { fill: var(--text-primary); fill-opacity: 0.04; }
path:hover, g:hover > rect { filter: brightness(1.08); }
details { margin: 8px 0; }
summary { cursor: pointer; color: var(--text-secondary); font-size: 14px; }
.scroll { overflow-x: auto; }
table { border-collapse: collapse; width: 100%; font-size: 13px; margin: 8px 0; }
table.narrow { width: auto; }
th, td { text-align: right; padding: 5px 8px; border-bottom: 1px solid var(--grid); white-space: nowrap; font-variant-numeric: tabular-nums; }
th[scope=row], thead th:first-child, td.tier, td.path, th.l { text-align: left; }
th[scope=row], td.path { white-space: normal; }
td.path { min-width: 10em; }
@media (max-width: 700px) { .opt { display: none; } td.path { min-width: 7em; } th, td { padding: 5px 4px; } table { font-size: 12px; } }
thead th { color: var(--text-secondary); font-weight: 600; border-bottom: 1px solid var(--axis); }
th[scope=row] { font-weight: 400; }
td.path { color: var(--text-secondary); }
td.share svg { vertical-align: middle; }
code { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 12.5px; }
"#;
