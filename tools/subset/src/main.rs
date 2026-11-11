//! `subset`: enforce and measure the Conductor Rust subset (docs/subset.md).

mod actor;
mod classify;
mod context;
mod facts;
mod history;
mod ids;
mod measure;
mod report;
mod rules;
mod score;
mod shape;
mod weights;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use serde::Deserialize;

use classify::{Occurrence, Rules};

const USAGE: &str = "usage:
  subset check [--config subset.toml]
  subset report [--config subset.toml] [--json]
  subset measure <paths...> [--json]
  subset score [paths...] [--config subset.toml] [--json] [--by crate|file|fn|construct]
  subset report-html --out <dir> [--config subset.toml] [--compare <dir>]... [--history <n>]
  subset ids
  subset score-ids
  subset rules";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[expect(dead_code, reason = "accepted so the file validates; nothing reads it")]
    version: u32,
    include: Vec<String>,
    allowed: BTreeSet<String>,
    #[serde(default)]
    derives: Vec<String>,
    #[serde(default)]
    macros: Vec<String>,
    #[serde(default)]
    test_macros: Vec<String>,
    /// Types of ours whose methods may take `&mut self` anywhere (proposal
    /// 0035).
    #[serde(default)]
    endpoints: Vec<String>,
    /// Methods whose closure argument may have a boolean expression as its
    /// body (proposals 0028, 0037).
    #[serde(default = "shape::default_predicate_adapters")]
    predicate_adapters: Vec<String>,
    /// Methods that may take a closure argument (`expr.closure.adapter`),
    /// besides the predicate adapters.
    #[serde(default = "shape::default_closure_methods")]
    closure_methods: Vec<String>,
    #[serde(default)]
    scope: Vec<Scope>,
    weights: Option<weights::Weights>,
}

/// Ids allowed only in the files under `paths`. Elsewhere a use of one is
/// reported with `rule` as its reason.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scope {
    rule: String,
    paths: Vec<String>,
    allowed: BTreeSet<String>,
}

impl Scope {
    fn covers(&self, path: &Path) -> Result<bool, Fatal> {
        Ok(Globs::new(&self.paths)?.cover(path))
    }
}

/// Path globs relative to the config's directory, where `*` stays within
/// one path component and `**` crosses any number.
struct Globs(Vec<glob::Pattern>);

impl Globs {
    fn new(patterns: &[String]) -> Result<Globs, Fatal> {
        patterns
            .iter()
            .map(|p| glob::Pattern::new(p).map_err(|e| Fatal(format!("path {p}: {e}"))))
            .collect::<Result<_, _>>()
            .map(Globs)
    }

    fn cover(&self, path: &Path) -> bool {
        let options = glob::MatchOptions {
            require_literal_separator: true,
            ..glob::MatchOptions::new()
        };
        self.0.iter().any(|p| p.matches_path_with(path, options))
    }
}

/// An error that ends the run with exit code 2.
struct Fatal(String);

struct Violation {
    path: String,
    id: String,
    reason: String,
    line: usize,
    col: usize,
}

impl Violation {
    fn location(&self) -> String {
        format!("{}:{}:{}", self.path, self.line, self.col)
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(Fatal(message)) => {
            eprintln!("subset: {message}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[String]) -> Result<ExitCode, Fatal> {
    let (command, rest) = args.split_first().ok_or_else(|| Fatal(USAGE.to_string()))?;
    let mut config = PathBuf::from("subset.toml");
    let mut json = false;
    let mut by = score::Level::Crate;
    let mut out = None;
    let mut compare = Vec::new();
    let mut history = None;
    let mut paths = Vec::new();
    let mut rest = rest.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--config" => {
                let value = rest.next().ok_or_else(|| Fatal(USAGE.to_string()))?;
                config = PathBuf::from(value);
            }
            "--json" => json = true,
            "--out" => {
                out = Some(PathBuf::from(
                    rest.next().ok_or_else(|| Fatal(USAGE.to_string()))?,
                ))
            }
            "--compare" => compare.push(PathBuf::from(
                rest.next().ok_or_else(|| Fatal(USAGE.to_string()))?,
            )),
            "--history" => {
                let value = rest.next().ok_or_else(|| Fatal(USAGE.to_string()))?;
                history = Some(
                    value
                        .parse()
                        .map_err(|_| Fatal(format!("--history takes a count, not {value}")))?,
                );
            }
            "--by" => {
                let value = rest.next().ok_or_else(|| Fatal(USAGE.to_string()))?;
                by = score::Level::parse(value)
                    .ok_or_else(|| Fatal(format!("unknown level {value}\n{USAGE}")))?;
            }
            flag if flag.starts_with("--") => {
                return Err(Fatal(format!("unknown flag {flag}\n{USAGE}")));
            }
            path => paths.push(PathBuf::from(path)),
        }
    }
    match command.as_str() {
        "check" => check(&config),
        "report" => report(&config, json),
        "measure" if !paths.is_empty() => measure(&paths, json),
        "score" => score_command(&config, &paths, by, json),
        "report-html" => match out {
            Some(out) => report_html(&config, &out, &compare, history),
            None => Err(Fatal(format!("report-html needs --out <dir>\n{USAGE}"))),
        },
        "ids" => {
            print!("{}", ids::markdown_table());
            Ok(ExitCode::SUCCESS)
        }
        "score-ids" => {
            print!("{}", weights::score_ids_table());
            Ok(ExitCode::SUCCESS)
        }
        "rules" => {
            print!("{}", rules::markdown_table());
            Ok(ExitCode::SUCCESS)
        }
        _ => Err(Fatal(USAGE.to_string())),
    }
}

fn check(config: &Path) -> Result<ExitCode, Fatal> {
    let violations = violations(config)?;
    for v in &violations {
        println!("{}: {}: {}", v.location(), v.id, v.reason);
    }
    Ok(if violations.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn report(config: &Path, json: bool) -> Result<ExitCode, Fatal> {
    let mut by_id: BTreeMap<String, Vec<Violation>> = BTreeMap::new();
    for v in violations(config)? {
        by_id.entry(v.id.clone()).or_default().push(v);
    }
    let mut rows: Vec<(String, Vec<Violation>)> = by_id.into_iter().collect();
    rows.sort_by(|(a_id, a), (b_id, b)| b.len().cmp(&a.len()).then(a_id.cmp(b_id)));
    let summarise = |seen: &[Violation]| {
        let files: BTreeSet<&str> = seen.iter().map(|v| v.path.as_str()).collect();
        let examples: Vec<String> = seen.iter().take(5).map(Violation::location).collect();
        (files.len(), examples)
    };
    if json {
        let ids: Vec<serde_json::Value> = rows
            .iter()
            .map(|(id, seen)| {
                let (files, examples) = summarise(seen);
                serde_json::json!({
                    "id": id,
                    "reason": seen[0].reason,
                    "count": seen.len(),
                    "files": files,
                    "examples": examples,
                })
            })
            .collect();
        let total: usize = rows.iter().map(|(_, seen)| seen.len()).sum();
        let out = serde_json::json!({ "total": total, "ids": ids });
        println!(
            "{}",
            serde_json::to_string_pretty(&out).expect("json value")
        );
    } else {
        for (id, seen) in &rows {
            let (files, examples) = summarise(seen);
            let noun = if files == 1 { "file" } else { "files" };
            println!(
                "{id} {} in {files} {noun}: {}",
                seen.len(),
                examples.join(" ")
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The files named, and every `*.rs` file under the directories named.
fn rust_files(paths: &[PathBuf]) -> Result<BTreeSet<PathBuf>, Fatal> {
    let mut files = BTreeSet::new();
    for path in paths {
        if path.is_dir() {
            files.extend(glob_files(&path.join("**/*.rs"))?);
        } else {
            files.insert(path.clone());
        }
    }
    Ok(files)
}

fn measure(paths: &[PathBuf], json: bool) -> Result<ExitCode, Fatal> {
    let files = rust_files(paths)?;
    let mut rows = Vec::new();
    let mut total = measure::Volume::default();
    for file in &files {
        let src = read(file)?;
        let volume = measure::measure(&src)
            .map_err(|e| Fatal(format!("{}: cannot lex: {e}", display(file))))?;
        total.tokens += volume.tokens;
        total.lines += volume.lines;
        rows.push((display(file), volume));
    }
    if json {
        let files: Vec<serde_json::Value> = rows
            .iter()
            .map(|(path, v)| serde_json::json!({ "path": path, "tokens": v.tokens, "lines": v.lines }))
            .collect();
        let out = serde_json::json!({
            "files": files,
            "total": { "tokens": total.tokens, "lines": total.lines },
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&out).expect("json value")
        );
    } else {
        for (path, v) in &rows {
            println!("{path}: {} tokens, {} lines", v.tokens, v.lines);
        }
        println!("total: {} tokens, {} lines", total.tokens, total.lines);
    }
    Ok(ExitCode::SUCCESS)
}

fn score_command(
    config_path: &Path,
    paths: &[PathBuf],
    by: score::Level,
    json: bool,
) -> Result<ExitCode, Fatal> {
    let config = load(config_path)?;
    let root = root_of(config_path);
    let files = match paths.is_empty() {
        true => included(&config, root)?,
        false => rust_files(paths)?,
    };
    let has_manifest = |dir: &Path| root.join(dir).join("Cargo.toml").is_file();
    let (scored, weights) = scored(&config, config_path, sources(root, &files)?, &has_manifest)?;
    let w = &weights.tiers;
    let rows = score::rows(&scored, by, w);
    let total = score::total(&scored);
    match json {
        true => println!(
            "{}",
            serde_json::to_string_pretty(&score::json(&rows, &total, by, w)).expect("json value")
        ),
        false => print!("{}", score::text(&rows, &total, by, w)),
    }
    Ok(ExitCode::SUCCESS)
}

fn report_html(
    config_path: &Path,
    out: &Path,
    compare: &[PathBuf],
    history: Option<usize>,
) -> Result<ExitCode, Fatal> {
    let config = load(config_path)?;
    let root = root_of(config_path);
    let has_manifest = |dir: &Path| root.join(dir).join("Cargo.toml").is_file();
    let (files, weights) = scored(
        &config,
        config_path,
        sources(root, &included(&config, root)?)?,
        &has_manifest,
    )?;
    let mut groups = vec![report::Group {
        title: config.include.join(", "),
        note: "The files under <code>include</code> in the config: the code written in the subset."
            .to_string(),
        comparison: false,
        files,
    }];
    for dir in compare {
        let (files, _) = scored(
            &config,
            config_path,
            sources(root, &rust_files(std::slice::from_ref(dir))?)?,
            &has_manifest,
        )?;
        groups.push(report::Group {
            title: format!("{}/", display(dir).trim_end_matches('/')),
            note: "Outside <code>include</code>, scored under the same rules for comparison. \
                   Code the subset doesn't cover counts as violations."
                .to_string(),
            comparison: true,
            files,
        });
    }
    let history = match history {
        Some(n) => snapshots(&config, config_path, root, n),
        None => Err("History wasn't generated. Pass --history <n> to add it.".to_string()),
    };
    let stamp = match history::head(root) {
        Some(head) => format!("{} at {head}.", config.include.join(", ")),
        None => format!("{}.", config.include.join(", ")),
    };
    let html = report::render(&report::Page {
        stamp,
        groups: &groups,
        history,
        weights: &weights.tiers,
    });
    std::fs::create_dir_all(out).map_err(|e| Fatal(format!("{}: {e}", display(out))))?;
    let index = out.join("index.html");
    std::fs::write(&index, html).map_err(|e| Fatal(format!("{}: {e}", display(&index))))?;
    Ok(ExitCode::SUCCESS)
}

/// The score per crate at each of the last `n` commits that changed files
/// under `include`, under today's config.
fn snapshots(
    config: &Config,
    config_path: &Path,
    root: &Path,
    n: usize,
) -> Result<Vec<report::Snapshot>, String> {
    let include = Globs::new(&config.include).map_err(|Fatal(e)| e)?;
    let mut out = Vec::new();
    for commit in history::commits(root, &config.include, n)? {
        let tree = history::tree(root, &commit.sha)?;
        let manifests: BTreeSet<&Path> = tree
            .iter()
            .filter(|p| p.file_name().is_some_and(|n| n == "Cargo.toml"))
            .filter_map(|p| p.parent())
            .collect();
        let rust: Vec<PathBuf> = tree.iter().filter(|p| include.cover(p)).cloned().collect();
        let sources = history::read(root, &commit.sha, &rust)?;
        let has_manifest = |dir: &Path| manifests.contains(dir);
        match scored(config, config_path, sources, &has_manifest) {
            Ok((files, weights)) => out.push(report::Snapshot {
                crates: score::rows(&files, score::Level::Crate, &weights.tiers)
                    .iter()
                    .map(|r| (r.name.clone(), r.score(&weights.tiers)))
                    .collect(),
                sha: commit.sha,
                date: commit.date,
            }),
            Err(Fatal(e)) => eprintln!("subset: history: skipping {}: {e}", commit.sha),
        }
    }
    Ok(out)
}

/// Scores `sources` under the config's allowed ids and weights.
fn scored<'c>(
    config: &'c Config,
    config_path: &Path,
    sources: Vec<(PathBuf, String)>,
    has_manifest: &dyn Fn(&Path) -> bool,
) -> Result<(Vec<score::ScoredFile>, &'c weights::Weights), Fatal> {
    let weights = config
        .weights
        .as_ref()
        .ok_or_else(|| Fatal(format!("{}: no [weights] table", display(config_path))))?;
    let typing = weights.typing().map_err(Fatal)?;
    let external = Globs::new(&weights.external_protocols)?;
    let (analysed, facts) = analyse(config, sources)?;
    let mut out = Vec::new();
    for a in analysed {
        let starts = measure::token_starts(&a.src)
            .map_err(|e| Fatal(format!("{}: cannot lex: {e}", display(&a.path))))?;
        let context = context::gather(&a.parsed, &facts, &typing, external.cover(&a.path));
        let judged = judge(&a, &context, weights);
        out.push(score::ScoredFile {
            path: display(&a.path),
            krate: crate_of(&a.path, has_manifest),
            tally: score::tally(a.src.len(), &starts, &judged, &context),
            violations: a.violations.len(),
        });
    }
    Ok((out, weights))
}

/// The id and tier each occurrence is charged at: `outside` for a violation,
/// `boundary` where an item we don't define fixes its shape, and otherwise
/// the tier `[weights]` gives its id.
fn judge(
    a: &Analysed,
    context: &context::Context,
    weights: &weights::Weights,
) -> Vec<score::Judged> {
    let wire = |extent: &std::ops::Range<usize>| {
        context
            .wire_fields
            .iter()
            .any(|f| f.start <= extent.start && extent.end <= f.end)
    };
    let fields = context.wire_fields.iter().map(|f| score::Judged {
        id: weights::WIRE_FIELD.to_string(),
        tier: weights::Tier::Boundary,
        extent: f.clone(),
    });
    a.occurrences
        .iter()
        .zip(&a.violating)
        .map(|(o, violating)| {
            let id = context
                .refinements
                .iter()
                .find(|r| r.from.covers(&o.id) && r.extent == o.extent)
                .map_or(o.id.as_str(), |r| r.to);
            let tier = weights.tier(id);
            let fixed = a
                .boundary
                .iter()
                .filter(|m| m.extent == o.extent)
                .map(|m| m.when)
                .min_by_key(|when| match when {
                    shape::Fixed::Always => 0,
                    shape::Fixed::IfRestricted => 1,
                });
            let tier = match (violating, fixed) {
                (true, _) => weights::Tier::Outside,
                (false, _) if wire(&o.extent) => weights::Tier::Boundary,
                (false, Some(shape::Fixed::Always)) => weights::Tier::Boundary,
                (false, Some(shape::Fixed::IfRestricted))
                    if tier.restricted() && !context.kept.contains(&o.extent) =>
                {
                    weights::Tier::Boundary
                }
                _ => tier,
            };
            score::Judged {
                id: id.to_string(),
                tier,
                extent: o.extent.clone(),
            }
        })
        .chain(fields)
        .collect()
}

/// The nearest directory above `path` that holds a `Cargo.toml`, or `.`
/// when there is none.
fn crate_of(path: &Path, has_manifest: &dyn Fn(&Path) -> bool) -> String {
    path.ancestors()
        .skip(1)
        .find(|dir| has_manifest(dir))
        .filter(|dir| !dir.as_os_str().is_empty())
        .map_or_else(|| ".".to_string(), display)
}

/// Every disallowed construct under `include`, in path and position order.
fn violations(config_path: &Path) -> Result<Vec<Violation>, Fatal> {
    let config = load(config_path)?;
    let root = root_of(config_path);
    let files = included(&config, root)?;
    let (analysed, _) = analyse(&config, sources(root, &files)?)?;
    Ok(analysed.into_iter().flat_map(|a| a.violations).collect())
}

fn load(config_path: &Path) -> Result<Config, Fatal> {
    let text = read(config_path)?;
    toml::from_str(&text).map_err(|e| Fatal(format!("{}: {e}", display(config_path))))
}

/// The directory paths in the config are relative to.
fn root_of(config_path: &Path) -> &Path {
    match config_path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

fn included(config: &Config, root: &Path) -> Result<BTreeSet<PathBuf>, Fatal> {
    let mut files = BTreeSet::new();
    for pattern in &config.include {
        files.extend(glob_files(&root.join(pattern))?);
    }
    Ok(files)
}

/// One file as `check` sees it, with what the score needs on top.
struct Analysed {
    /// Relative to the config's directory.
    path: PathBuf,
    src: String,
    occurrences: Vec<Occurrence>,
    /// Per occurrence, whether it is a violation: an id not allowed here, or
    /// an allowed id that breaks a shape rule.
    violating: Vec<bool>,
    violations: Vec<Violation>,
    boundary: Vec<shape::Mark>,
    parsed: syn::File,
}

/// Each file's path relative to `root`, and its text.
fn sources(root: &Path, files: &BTreeSet<PathBuf>) -> Result<Vec<(PathBuf, String)>, Fatal> {
    files
        .iter()
        .map(|file| {
            let relative = file.strip_prefix(root).unwrap_or(file).to_path_buf();
            Ok((relative, read(file)?))
        })
        .collect()
}

/// Every file's analysis, and the facts gathered across them. Paths are
/// relative to the config's directory.
fn analyse(
    config: &Config,
    sources: Vec<(PathBuf, String)>,
) -> Result<(Vec<Analysed>, facts::Facts), Fatal> {
    let rules = Rules {
        derives: &config.derives,
        macros: &config.macros,
        test_macros: &config.test_macros,
    };
    let mut parsed_files = Vec::new();
    let mut parse_errors = Vec::new();
    for (relative_path, src) in sources {
        match syn::parse_file(&src) {
            Ok(parsed) => parsed_files.push((relative_path, src, parsed)),
            Err(e) => {
                let start = e.span().start();
                parse_errors.push(format!(
                    "{}:{}:{}: parse error: {e}",
                    display(&relative_path),
                    start.line,
                    start.column + 1
                ));
            }
        }
    }
    if !parse_errors.is_empty() {
        return Err(Fatal(parse_errors.join("\n")));
    }
    let facts = facts::Facts::gather(
        parsed_files
            .iter()
            .map(|(path, _, parsed)| (path.as_path(), parsed)),
    );
    let mut out = Vec::new();
    for (relative_path, src, parsed) in parsed_files {
        let relative_path = relative_path.as_path();
        let relative = display(relative_path);
        let kind = shape::FileKind {
            test: relative_path
                .components()
                .any(|c| c == Component::Normal("tests".as_ref())),
            crate_root: crate_root(relative_path),
        };
        let allowed = Allowed::for_file(config, relative_path)?;
        let occurrences = classify::classify(&parsed, &rules, kind.test);
        let mut violating: Vec<bool> = occurrences
            .iter()
            .map(|o| !allowed.contains(&o.id))
            .collect();
        let plain = occurrences
            .iter()
            .filter(|o| !allowed.contains(&o.id))
            .map(|o| Violation {
                path: relative.clone(),
                reason: allowed.reason(o),
                id: o.id.clone(),
                line: o.line,
                col: o.col,
            });
        let lists = shape::Lists {
            endpoints: &config.endpoints,
            predicate_adapters: &config.predicate_adapters,
            closure_methods: &config.closure_methods,
        };
        let shape = shape::check(&parsed, &kind, &facts, &lists);
        let findings: Vec<&shape::Finding> = shape
            .findings
            .iter()
            .filter(|f| allowed.contains(f.rule.id))
            .collect();
        for finding in &findings {
            if let Some(i) = found_at(&occurrences, finding) {
                violating[i] = true;
            }
        }
        let shaped = findings.iter().map(|f| Violation {
            path: relative.clone(),
            id: f.rule.id.to_string(),
            reason: f.rule.message.to_string(),
            line: f.line,
            col: f.col,
        });
        let mut violations: Vec<Violation> = plain.chain(shaped).collect();
        violations.sort_by_key(|v| (v.line, v.col));
        out.push(Analysed {
            path: relative_path.to_path_buf(),
            src,
            occurrences,
            violating,
            violations,
            boundary: shape.boundary,
            parsed,
        });
    }
    Ok((out, facts))
}

/// The occurrence a shape finding is about: the one of its id at its
/// position, or else the smallest one of its id that contains it.
fn found_at(occurrences: &[Occurrence], finding: &shape::Finding) -> Option<usize> {
    let same_id = || {
        occurrences
            .iter()
            .enumerate()
            .filter(|(_, o)| o.id == finding.rule.id)
    };
    same_id()
        .find(|(_, o)| (o.line, o.col) == (finding.line, finding.col))
        .or_else(|| {
            same_id()
                .filter(|(_, o)| o.extent.contains(&finding.at))
                .min_by_key(|(_, o)| o.extent.len())
        })
        .map(|(i, _)| i)
}

/// `src/lib.rs` or `src/main.rs`: the files whose inner attributes are crate
/// attributes.
fn crate_root(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str());
    let parent = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str());
    matches!((parent, name), (Some("src"), Some("lib.rs" | "main.rs")))
}

/// The ids one file may use: the global list and every scope that covers the
/// file. Scopes that don't cover it give the reason for their ids.
struct Allowed<'c> {
    global: &'c BTreeSet<String>,
    here: Vec<&'c Scope>,
    elsewhere: Vec<&'c Scope>,
}

impl<'c> Allowed<'c> {
    fn for_file(config: &'c Config, path: &Path) -> Result<Allowed<'c>, Fatal> {
        let mut here = Vec::new();
        let mut elsewhere = Vec::new();
        for scope in &config.scope {
            match scope.covers(path)? {
                true => here.push(scope),
                false => elsewhere.push(scope),
            }
        }
        Ok(Allowed {
            global: &config.allowed,
            here,
            elsewhere,
        })
    }

    fn contains(&self, id: &str) -> bool {
        self.global.contains(id) || self.here.iter().any(|s| s.allowed.contains(id))
    }

    fn reason(&self, occurrence: &Occurrence) -> String {
        let scoped = self
            .elsewhere
            .iter()
            .find(|s| s.allowed.contains(&occurrence.id))
            .map(|s| s.rule.as_str());
        scoped
            .or_else(|| rules::forbidden(&occurrence.id))
            .unwrap_or(occurrence.reason)
            .to_string()
    }
}

fn glob_files(pattern: &Path) -> Result<Vec<PathBuf>, Fatal> {
    let pattern = pattern.to_string_lossy();
    let paths = glob::glob(&pattern).map_err(|e| Fatal(format!("include {pattern}: {e}")))?;
    paths
        .map(|entry| entry.map_err(|e| Fatal(format!("include {pattern}: {e}"))))
        .filter(|entry| entry.as_ref().map_or(true, |p| p.is_file()))
        .collect()
}

fn read(path: &Path) -> Result<String, Fatal> {
    std::fs::read_to_string(path).map_err(|e| Fatal(format!("{}: {e}", display(path))))
}

fn display(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new proposal can't forget its weight: every id the repository's
    /// config allows, and every score-only id, has an explicit tier.
    #[test]
    fn every_allowed_id_has_a_weight() {
        let config = load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../subset.toml"))
            .unwrap_or_else(|Fatal(e)| panic!("{e}"));
        let weights = config.weights.as_ref().expect("[weights] in subset.toml");
        let scoped = config.scope.iter().flat_map(|s| &s.allowed);
        let score_ids = weights::SCORE_IDS.iter().map(|(id, _)| id.to_string());
        let missing: Vec<String> = config
            .allowed
            .iter()
            .chain(scoped)
            .cloned()
            .chain(score_ids)
            .filter(|id| !weights.ids.contains_key(id))
            .collect();
        assert_eq!(missing, Vec::<String>::new());
    }
}
