use alloc::string::String;

pub const LIST: &str =
    "home issue list <project> [--offline] [--data-dir <dir>]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IssueQuery<'a> {
    pub project: &'a str,
    pub offline: bool,
    pub data: Option<&'a str>,
}

#[must_use]
pub fn issue_query(
    args: &[String],
) -> Option<Result<IssueQuery<'_>, &'static str>> {
    match args {
        [group, command, rest @ ..]
            if group == "issue" && command == "list" =>
        {
            Some(parse(rest).ok_or(LIST))
        }
        _ => None,
    }
}

fn parse(args: &[String]) -> Option<IssueQuery<'_>> {
    let (project, mut flags) = args.split_first()?;
    let mut query = IssueQuery {
        project: argument(project)?,
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
