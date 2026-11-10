//! `subset`: enforce and measure the Conductor Rust subset (docs/subset.md).

mod classify;
mod ids;
mod measure;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use serde::Deserialize;

use classify::{Occurrence, Rules};

const USAGE: &str = "usage:
  subset check [--config subset.toml]
  subset report [--config subset.toml] [--json]
  subset measure <paths...> [--json]
  subset ids";

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
}

/// An error that ends the run with exit code 2.
struct Fatal(String);

struct Violation {
    path: String,
    occurrence: Occurrence,
}

impl Violation {
    fn location(&self) -> String {
        format!(
            "{}:{}:{}",
            self.path, self.occurrence.line, self.occurrence.col
        )
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
        _ => Err(Fatal(USAGE.to_string())),
    }
}

fn check(config: &Path) -> Result<ExitCode, Fatal> {
    let violations = violations(config)?;
    for v in &violations {
        println!(
            "{}: {}: {}",
            v.location(),
            v.occurrence.id,
            v.occurrence.reason
        );
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
        by_id.entry(v.occurrence.id.clone()).or_default().push(v);
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
                    "reason": seen[0].occurrence.reason,
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
    let mut out = Vec::new();
    let mut parse_errors = Vec::new();
    for file in &files {
        let relative_path = file.strip_prefix(root).unwrap_or(file);
        let relative = display(relative_path);
        let src = read(file)?;
        let parsed = match syn::parse_file(&src) {
            Ok(parsed) => parsed,
            Err(e) => {
                let start = e.span().start();
                parse_errors.push(format!(
                    "{relative}:{}:{}: parse error: {e}",
                    start.line,
                    start.column + 1
                ));
                continue;
            }
        };
        let test_file = relative_path
            .components()
            .any(|c| c == Component::Normal("tests".as_ref()));
        let mut found = classify::classify(&parsed, &rules, test_file);
        found.sort_by_key(|o| (o.line, o.col));
        out.extend(
            found
                .into_iter()
                .filter(|o| !config.allowed.contains(&o.id))
                .map(|occurrence| Violation {
                    path: relative.clone(),
                    occurrence,
                }),
        );
    }
    if parse_errors.is_empty() {
        Ok(out)
    } else {
        Err(Fatal(parse_errors.join("\n")))
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
