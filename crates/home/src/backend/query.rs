use alloc::string::String;

pub const LIST: &str =
    "home issue list <project> [--offline] [--data-dir <dir>]";
pub const SHOW: &str =
    "home issue show <project> <reference> [--offline] [--data-dir <dir>]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IssueQuery<'a> {
    pub project: &'a str,
    pub reference: Option<&'a str>,
    pub offline: bool,
    pub data: Option<&'a str>,
}

#[must_use]
pub fn issue_query(
    args: &[String],
) -> Option<Result<IssueQuery<'_>, &'static str>> {
    let [group, command, rest @ ..] = args else {
        return None;
    };
    if group != "issue" {
        return None;
    }
    let usage = match command.as_str() {
        "list" => LIST,
        "show" => SHOW,
        _ => return None,
    };
    Some(parse(rest, command == "show").ok_or(usage))
}

fn parse(args: &[String], show: bool) -> Option<IssueQuery<'_>> {
    let (project, tail) = args.split_first()?;
    let (reference, mut flags) = if show {
        let (reference, rest) = tail.split_first()?;
        (Some(argument(reference)?), rest)
    } else {
        (None, tail)
    };
    let mut query = IssueQuery {
        project: argument(project)?,
        reference,
        offline: false,
        data: None,
    };
    while !flags.is_empty() {
        flags = match flags {
            [flag, rest @ ..] if flag == "--offline" && !query.offline => {
                query.offline = true;
                rest
            }
            [flag, dir, rest @ ..]
                if flag == "--data-dir" && query.data.is_none() =>
            {
                query.data = Some(argument(dir)?);
                rest
            }
            _ => return None,
        }
    }
    Some(query)
}

fn argument(word: &str) -> Option<&str> {
    (!word.is_empty() && !word.starts_with("--")).then_some(word)
}
