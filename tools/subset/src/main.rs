//! `subset`: enforce and measure the Conductor Rust subset (docs/subset.md).

mod actor;
mod classify;
mod facts;
mod ids;
mod measure;
mod rules;
mod shape;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use serde::Deserialize;

use classify::{Occurrence, Rules};

const USAGE: &str = "usage:
  subset check [--config subset.toml]
  subset report [--config subset.toml] [--json]
  subset measure <paths...> [--json]
  subset ids
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
    #[serde(default)]
    scope: Vec<Scope>,
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
        let options = glob::MatchOptions {
            require_literal_separator: true,
            ..glob::MatchOptions::new()
        };
        for pattern in &self.paths {
            let pattern = glob::Pattern::new(pattern)
                .map_err(|e| Fatal(format!("scope path {pattern}: {e}")))?;
            if pattern.matches_path_with(path, options) {
                return Ok(true);
            }
        }
        Ok(false)
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
    let mut paths = Vec::new();
    let mut rest = rest.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--config" => {
                let value = rest.next().ok_or_else(|| Fatal(USAGE.to_string()))?;
                config = PathBuf::from(value);
            }
            "--json" => json = true,
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
        "ids" => {
            print!("{}", ids::markdown_table());
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

fn measure(paths: &[PathBuf], json: bool) -> Result<ExitCode, Fatal> {
    let mut files = BTreeSet::new();
    for path in paths {
        if path.is_dir() {
            files.extend(glob_files(&path.join("**/*.rs"))?);
        } else {
            files.insert(path.clone());
        }
    }
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

/// Every disallowed construct under `include`, in path and position order.
fn violations(config_path: &Path) -> Result<Vec<Violation>, Fatal> {
    let text = read(config_path)?;
    let config: Config =
        toml::from_str(&text).map_err(|e| Fatal(format!("{}: {e}", display(config_path))))?;
    let root = config_path.parent().unwrap_or(Path::new(""));
    let root = if root.as_os_str().is_empty() {
        Path::new(".")
    } else {
        root
    };
    let mut files = BTreeSet::new();
    for pattern in &config.include {
        files.extend(glob_files(&root.join(pattern))?);
    }
    let rules = Rules {
        derives: &config.derives,
        macros: &config.macros,
        test_macros: &config.test_macros,
    };
    let mut parsed_files = Vec::new();
    let mut parse_errors = Vec::new();
    for file in &files {
        let relative_path = file.strip_prefix(root).unwrap_or(file);
        let src = read(file)?;
        match syn::parse_file(&src) {
            Ok(parsed) => parsed_files.push((relative_path, parsed)),
            Err(e) => {
                let start = e.span().start();
                parse_errors.push(format!(
                    "{}:{}:{}: parse error: {e}",
                    display(relative_path),
                    start.line,
                    start.column + 1
                ));
            }
        }
    }
    if !parse_errors.is_empty() {
        return Err(Fatal(parse_errors.join("\n")));
    }
    let facts = facts::Facts::gather(parsed_files.iter().map(|(path, parsed)| (*path, parsed)));
    let mut out = Vec::new();
    for (relative_path, parsed) in &parsed_files {
        let relative = display(relative_path);
        let kind = shape::FileKind {
            test: relative_path
                .components()
                .any(|c| c == Component::Normal("tests".as_ref())),
            crate_root: crate_root(relative_path),
        };
        let allowed = Allowed::for_file(&config, relative_path)?;
        let plain = classify::classify(parsed, &rules, kind.test)
            .into_iter()
            .filter(|o| !allowed.contains(&o.id))
            .map(|o| Violation {
                path: relative.clone(),
                reason: allowed.reason(&o),
                id: o.id,
                line: o.line,
                col: o.col,
            });
        let shaped = shape::check(parsed, &kind, &facts)
            .into_iter()
            .filter(|f| allowed.contains(f.rule.id))
            .map(|f| Violation {
                path: relative.clone(),
                id: f.rule.id.to_string(),
                reason: f.rule.message.to_string(),
                line: f.line,
                col: f.col,
            });
        let mut found: Vec<Violation> = plain.chain(shaped).collect();
        found.sort_by_key(|v| (v.line, v.col));
        out.extend(found);
    }
    Ok(out)
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
