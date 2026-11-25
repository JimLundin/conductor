mod instruction;
pub use instruction::{Index, Instruction, Reply, Revision};
use vcs::{DiffChange, DiffExtent, Entry, Error, FilePatch, Merge};

pub struct Output {
    pub bytes: Vec<u8>,
    pub success: bool,
    pub error: String,
}

impl Output {
    pub const CAPTURE_TO_SEE_AN_OVERSIZED_PATCH: usize =
        vcs::plan::PATCH_LIMIT + 1;
}

pub struct Command {
    pub args: Vec<String>,
    pub index: Index,
    pub identity: bool,
}

#[must_use]
pub fn command(request: &Instruction) -> Command {
    Command {
        args: args(request),
        index: index(request),
        identity: matches!(request, Instruction::Commit { .. }),
    }
}

const fn index(request: &Instruction) -> Index {
    match request {
        Instruction::ReadTree { index, .. }
        | Instruction::Stage { index }
        | Instruction::WriteTree { index } => *index,
        _ => Index::Working,
    }
}

fn words(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| String::from(*arg)).collect()
}

fn revision(revision: &Revision) -> String {
    match revision {
        Revision::Head => String::from("HEAD"),
        Revision::Named(name) => name.clone(),
        Revision::Commit(name) => format!("{name}^{{commit}}"),
        Revision::Tree(name) => format!("{name}^{{tree}}"),
    }
}

fn args(request: &Instruction) -> Vec<String> {
    match request {
        Instruction::Root | Instruction::ProbeRoot => {
            words(&["rev-parse", "--show-toplevel"])
        }
        Instruction::CommittedHead => words(&[
            "rev-parse",
            "--verify",
            &revision(&Revision::Commit(String::from("HEAD"))),
        ]),
        Instruction::GitDirectory => {
            words(&["rev-parse", "--absolute-git-dir"])
        }
        Instruction::CommonDirectory => {
            words(&["rev-parse", "--path-format=absolute", "--git-common-dir"])
        }
        Instruction::RootCommits => {
            words(&["rev-list", "--max-parents=0", "--first-parent", "HEAD"])
        }
        Instruction::Resolve(value) => {
            words(&["rev-parse", "--verify", &revision(value)])
        }
        Instruction::Subtree { commit, path } => {
            words(&["cat-file", "-t", &format!("{commit}:{path}")])
        }
        Instruction::AddWorktree {
            branch,
            path,
            start,
        } => words(&["worktree", "add", "-b", branch, path, start]),
        Instruction::HeadReference => words(&["symbolic-ref", "HEAD"]),
        Instruction::ReadTree { revision, .. } => {
            words(&["read-tree", revision])
        }
        Instruction::Stage { .. } => words(&["add", "-A", "--", "."]),
        Instruction::WriteTree { .. } => words(&["write-tree"]),
        Instruction::Commit {
            tree,
            parents,
            message,
        } => commit(tree, parents, message),
        Instruction::UpdateReference {
            reference,
            commit,
            expected,
        } => update(reference, commit, expected.as_deref()),
        Instruction::DeleteReference { reference } => {
            words(&["update-ref", "-d", reference])
        }
        Instruction::DiffNames { previous, commit } => {
            diff(previous, commit, None)
        }
        Instruction::DiffPatch {
            previous,
            commit,
            path,
        } => diff(previous, commit, Some(path)),
        Instruction::RemoveWorktree { path } => {
            words(&["worktree", "remove", "--force", path])
        }
        Instruction::PruneWorktrees => words(&["worktree", "prune"]),
        Instruction::References { prefix, limit } => words(&[
            "for-each-ref",
            &format!("--count={limit}"),
            "--format=%(refname)",
            prefix,
        ]),
        Instruction::Origin { push } => origin(*push),
        Instruction::Push { branch } => {
            words(&["push", "--quiet", "origin", &format!("{branch}:{branch}")])
        }
        Instruction::CountAhead { base, tip } => {
            words(&["rev-list", "--count", &format!("{base}..{tip}")])
        }
        Instruction::Ancestor { base, tip } => {
            words(&["merge-base", "--is-ancestor", base, tip])
        }
        Instruction::Worktrees => {
            words(&["worktree", "list", "--porcelain", "-z"])
        }
        Instruction::Merge { base, tip } => words(&[
            "merge-tree",
            "--write-tree",
            "--name-only",
            "--no-messages",
            base,
            tip,
        ]),
        Instruction::Integrate { .. } => {
            words(&["update-ref", "--stdin", "-z"])
        }
    }
}

fn diff(previous: &str, commit: &str, path: Option<&str>) -> Vec<String> {
    let mut args =
        words(&["diff", "--no-ext-diff", "--no-textconv", "--no-renames"]);
    args.extend(match path {
        None => words(&["--name-status", "-z"]),
        Some(_) => words(&["--no-color"]),
    });
    args.extend(words(&[previous, commit, "--"]));
    args.extend(path.map(|entry_path| format!(":(literal){entry_path}")));
    args
}

fn origin(push: bool) -> Vec<String> {
    if push {
        words(&["remote", "get-url", "--push", "origin"])
    } else {
        words(&["remote", "get-url", "origin"])
    }
}

fn commit(tree: &str, parents: &[String], message: &str) -> Vec<String> {
    let mut args = words(&["commit-tree", tree]);
    for parent in parents {
        args.extend(words(&["-p", parent]));
    }
    args.extend(words(&["-m", message]));
    args
}

fn update(
    reference: &str,
    commit: &str,
    expected: Option<&str>,
) -> Vec<String> {
    let mut args = words(&["update-ref", reference, commit]);
    args.extend(expected.map(String::from));
    args
}

#[must_use]
pub fn probed(
    path: &str,
    top: Option<&str>,
    roots: Option<&str>,
    remote: Option<&str>,
) -> Option<vcs::discovery::Probe> {
    vcs::discovery::observed(
        String::from(path),
        top.map(str::trim_end),
        roots?.lines().map(str::to_owned).collect(),
        remote
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(str::to_owned),
    )
}

pub fn reply(request: &Instruction, output: Output) -> Result<Reply, Error> {
    match request {
        Instruction::ProbeRoot | Instruction::CommittedHead => {
            committed_repository(output, &context(request)).map(Reply::Text)
        }
        Instruction::HeadReference => {
            text(output, "symbolic-ref").map(|reference| {
                Reply::Text(
                    reference
                        .strip_prefix("refs/heads/")
                        .unwrap_or_default()
                        .to_owned(),
                )
            })
        }
        Instruction::Subtree { .. } => {
            text(output, "cat-file").map(|kind| Reply::Found(kind == "tree"))
        }
        Instruction::DiffNames { .. } => listing(output),
        Instruction::DiffPatch { .. } => patch(output).map(Reply::Patch),
        Instruction::Worktrees => text(output, "worktree")
            .map(|listed| Reply::Worktrees(worktrees(&listed))),
        Instruction::Merge { .. } => merge(&output).map(Reply::Merge),
        Instruction::Ancestor { .. } => Ok(Reply::Found(output.success)),
        Instruction::CountAhead { .. } => text(output, "rev-list")?
            .parse()
            .map(Reply::Count)
            .map_err(|_| Error::failed("landing", "Git counted no commits")),
        Instruction::References { .. } | Instruction::RootCommits => {
            text(output, "for-each-ref").map(|text| {
                Reply::References(text.lines().map(str::to_owned).collect())
            })
        }
        _ => text(output, &context(request)).map(Reply::Text),
    }
}

fn committed_repository(
    output: Output,
    context: &str,
) -> Result<String, Error> {
    if output.success {
        text(output, context)
    } else {
        Err(Error::NotRepository)
    }
}

fn context(request: &Instruction) -> String {
    command(request).args.first().cloned().unwrap_or_default()
}

fn checked(output: Output, context: &str) -> Result<Vec<u8>, Error> {
    match (output.bytes.len() > vcs::plan::PATCH_LIMIT, output.success) {
        (true, _) => Err(Error::failed(
            context,
            "Git output exceeded the capture limit",
        )),
        (false, true) => Ok(output.bytes),
        (false, false) => Err(Error::failed(context, &output.error)),
    }
}

pub fn text(output: Output, context: &str) -> Result<String, Error> {
    String::from_utf8(checked(output, context)?)
        .map(|text| text.trim_end_matches('\n').to_owned())
        .map_err(|error| {
            Error::failed(
                "Git path or metadata is not UTF-8",
                &error.to_string(),
            )
        })
}

fn listing(output: Output) -> Result<Reply, Error> {
    if output.bytes.len() > vcs::plan::PATCH_LIMIT {
        return Ok(Reply::Names {
            entries: Vec::new(),
            extent: DiffExtent::Truncated,
        });
    }
    let metadata = String::from_utf8(checked(output, "snapshot file list")?)
        .map_err(|error| {
            Error::failed("diff contains non-UTF-8 paths", &error.to_string())
        })?;
    let entries = entries(&metadata)?;
    Ok(bounded(entries))
}

fn bounded(entries: Vec<Entry>) -> Reply {
    let (kept, extent) = vcs::plan::bounded_entries(entries);
    Reply::Names {
        entries: kept,
        extent,
    }
}

fn entries(metadata: &str) -> Result<Vec<Entry>, Error> {
    match metadata {
        "" => Ok(Vec::new()),
        _ => metadata
            .trim_end_matches('\0')
            .split('\0')
            .collect::<Vec<_>>()
            .chunks(2)
            .map(entry)
            .collect(),
    }
}

fn entry(parts: &[&str]) -> Result<Entry, Error> {
    let status = parts
        .first()
        .ok_or_else(|| Error::failed("diff metadata", "missing status"))?;
    let path = parts
        .get(1)
        .ok_or_else(|| Error::failed("diff metadata", "missing path"))?;
    let change = match *status {
        "A" => DiffChange::Write,
        "D" => DiffChange::Delete,
        _ => DiffChange::Edit,
    };
    Ok(Entry {
        path: String::from(*path),
        change,
    })
}

fn patch(output: Output) -> Result<FilePatch, Error> {
    if output.bytes.len() > vcs::plan::PATCH_LIMIT {
        Ok(FilePatch::TooLarge)
    } else {
        Ok(decoded_patch(checked(output, "snapshot patch")?))
    }
}

fn decoded_patch(bytes: Vec<u8>) -> FilePatch {
    match String::from_utf8(bytes) {
        Ok(text) if text.contains("\nBinary files ") => FilePatch::Binary,
        Ok(text) => FilePatch::Text(text),
        Err(error) => FilePatch::Unsupported {
            reason: format!("patch is not UTF-8: {error}"),
        },
    }
}

fn merge(output: &Output) -> Result<Merge, Error> {
    let text = String::from_utf8_lossy(&output.bytes);
    let mut lines = text.lines();
    let tree = lines.next().unwrap_or_default();
    match (output.success, output.error.trim()) {
        (true, _) => Ok(Merge::Tree(String::from(tree))),
        (false, "") => Ok(Merge::Conflicts(lines.map(str::to_owned).collect())),
        (false, said) => Err(Error::failed(
            "landing",
            &format!("the branches couldn't be merged: {said}"),
        )),
    }
}

#[must_use]
pub fn worktrees(porcelain: &str) -> Vec<vcs::Checkout> {
    porcelain
        .split("\0\0")
        .filter_map(|record| {
            let mut lines = record.split('\0');
            let path = lines.next()?.strip_prefix("worktree ")?;
            let branch = lines
                .find_map(|line| line.strip_prefix("branch "))
                .map(str::to_owned);
            Some(vcs::Checkout {
                path: String::from(path),
                branch,
            })
        })
        .collect()
}

#[must_use]
pub fn origin_default(symbolic: &str) -> Option<String> {
    symbolic
        .trim_end()
        .strip_prefix("origin/")
        .filter(|branch| !branch.is_empty())
        .map(String::from)
}

fn parse_distance(ab: &str) -> Option<vcs::status::Distance> {
    let (ahead, behind) = ab.split_once(' ')?;
    Some(vcs::status::Distance {
        ahead: ahead.strip_prefix('+')?.parse().ok()?,
        behind: behind.strip_prefix('-')?.parse().ok()?,
    })
}

#[must_use]
pub fn status(porcelain: &str) -> vcs::status::Checkout {
    let (mut state, upstream, distance) = porcelain
        .lines()
        .filter(|line| !line.is_empty())
        .fold((vcs::status::Checkout::default(), None, None), status_line);
    state.upstream =
        upstream.map(|name| vcs::status::Upstream { name, distance });
    state
}

type Status = (
    vcs::status::Checkout,
    Option<String>,
    Option<vcs::status::Distance>,
);

fn status_line((mut state, upstream, distance): Status, line: &str) -> Status {
    if let Some(header) = line.strip_prefix("# ") {
        status_header((state, upstream, distance), header)
    } else {
        state.changed += 1;
        (state, upstream, distance)
    }
}

fn status_header(
    (mut state, mut upstream, mut distance): Status,
    header: &str,
) -> Status {
    let (key, value) = header.split_once(' ').unwrap_or((header, ""));
    match key {
        "branch.oid" => state.commit = named(value, "(initial)"),
        "branch.head" => state.branch = named(value, "(detached)"),
        "branch.upstream" => upstream = Some(String::from(value)),
        "branch.ab" => distance = parse_distance(value),
        _ => {}
    }
    (state, upstream, distance)
}

fn named(value: &str, missing: &str) -> Option<String> {
    (value != missing).then(|| String::from(value))
}

#[cfg(test)]
mod tests;

#[must_use]
pub fn full_commit(commit: &str) -> bool {
    matches!(commit.len(), 40 | 64)
        && commit.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[must_use]
pub fn head(branch: &str) -> String {
    format!("refs/heads/{branch}")
}

#[must_use]
pub fn tracking_branch(tracking: &str) -> Option<String> {
    tracking.strip_prefix("refs/remotes/origin/").map(head)
}
