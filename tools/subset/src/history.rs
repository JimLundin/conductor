//! Source files as they were at past commits, read through git.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct Commit {
    pub sha: String,
    /// The commit date, `YYYY-MM-DD`.
    pub date: String,
}

/// The last `n` commits that touched files matching `patterns`, oldest first.
pub fn commits(root: &Path, patterns: &[String], n: usize) -> Result<Vec<Commit>, String> {
    let mut args = vec![
        "log".to_string(),
        format!("-n{n}"),
        "--format=%H %cs".to_string(),
        "--".to_string(),
    ];
    args.extend(patterns.iter().map(|p| format!(":(glob){p}")));
    let out = git(root, &args)?;
    let mut commits: Vec<Commit> = out
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(sha, date)| Commit {
            sha: sha.to_string(),
            date: date.to_string(),
        })
        .collect();
    commits.reverse();
    Ok(commits)
}

/// Every path in the tree of `sha`, relative to the repository root.
pub fn tree(root: &Path, sha: &str) -> Result<Vec<PathBuf>, String> {
    let out = git(root, &["ls-tree", "-r", "--name-only", sha])?;
    Ok(out.lines().map(PathBuf::from).collect())
}

/// The text of each of `paths` at `sha`, read with one `git cat-file`.
pub fn read(root: &Path, sha: &str, paths: &[PathBuf]) -> Result<Vec<(PathBuf, String)>, String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("git cat-file: {e}"))?;
    let requests: String = paths
        .iter()
        .map(|p| format!("{sha}:./{}\n", p.to_string_lossy()))
        .collect();
    let mut stdin = child.stdin.take().expect("piped stdin");
    let writer = std::thread::spawn(move || stdin.write_all(requests.as_bytes()));
    let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
    let mut out = Vec::new();
    for path in paths {
        let mut header = String::new();
        stdout
            .read_line(&mut header)
            .map_err(|e| format!("git cat-file: {e}"))?;
        let size: usize = header
            .split_whitespace()
            .nth(2)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| format!("git cat-file: {}: {}", path.display(), header.trim()))?;
        let mut body = vec![0; size + 1];
        stdout
            .read_exact(&mut body)
            .map_err(|e| format!("git cat-file: {e}"))?;
        body.truncate(size);
        let text = String::from_utf8(body).map_err(|e| format!("{}: {e}", path.display()))?;
        out.push((path.clone(), text));
    }
    writer
        .join()
        .expect("writer thread")
        .map_err(|e| format!("git cat-file: {e}"))?;
    child.wait().map_err(|e| format!("git cat-file: {e}"))?;
    Ok(out)
}

/// The short sha and date of `HEAD`, if `root` is in a git repository.
pub fn head(root: &Path) -> Option<String> {
    git(root, &["log", "-1", "--format=%h, %cs"])
        .ok()
        .map(|s| s.trim().to_string())
}

fn git<S: AsRef<std::ffi::OsStr>>(root: &Path, args: &[S]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| format!("git: {e}"))?;
    match out.status.success() {
        true => String::from_utf8(out.stdout).map_err(|e| format!("git: {e}")),
        false => Err(format!(
            "git {}: {}",
            args.first()
                .map(|a| a.as_ref().to_string_lossy().to_string())
                .unwrap_or_default(),
            String::from_utf8_lossy(&out.stderr).trim()
        )),
    }
}
