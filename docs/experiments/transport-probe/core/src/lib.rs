#![no_std]

extern crate alloc;

use alloc::{format, string::String, vec::Vec};

pub const REF: &str = "refs/heads/main";
pub const ZERO: &str = "0000000000000000000000000000000000000000";
pub const TOKEN: &str = "fixture-token";
pub const BASIC: &str = "Basic eC1hY2Nlc3MtdG9rZW46Zml4dHVyZS10b2tlbg==";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    pub old: String,
    pub new: String,
    pub name: String,
    pub pack: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum GitAction {
    Challenge,
    Advertise,
    Receive(Update),
    Missing,
}

#[derive(Clone, Debug)]
pub struct Request {
    pub header: String,
    pub body: Vec<u8>,
}

#[must_use]
pub fn header_value<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    for line in header.lines() {
        if let Some((key, value)) = line.split_once(':')
            && key.eq_ignore_ascii_case(name)
        {
            return Some(value.trim());
        }
    }
    None
}

#[must_use]
pub fn content_length(header: &str) -> Option<usize> {
    header_value(header, "content-length")
        .unwrap_or("0")
        .parse()
        .ok()
}

#[derive(Clone, Copy, Debug)]
pub enum Body {
    Length(usize),
    Chunked,
}

#[must_use]
pub fn body_framing(header: &str) -> Option<Body> {
    if header_value(header, "transfer-encoding") == Some("chunked") {
        return Some(Body::Chunked);
    }
    content_length(header).map(Body::Length)
}

#[must_use]
pub fn chunk_length(line: &str) -> Option<usize> {
    usize::from_str_radix(line.trim(), 16).ok()
}

#[must_use]
pub fn git_action(request: &Request, authorization: &str) -> GitAction {
    if header_value(&request.header, "authorization") != Some(authorization) {
        return GitAction::Challenge;
    }
    if request
        .header
        .starts_with("GET /repo.git/info/refs?service=git-receive-pack ")
    {
        return GitAction::Advertise;
    }
    if request
        .header
        .starts_with("POST /repo.git/git-receive-pack ")
        && let Some(update) = parse_update(&request.body)
    {
        return GitAction::Receive(update);
    }
    GitAction::Missing
}

#[must_use]
pub fn parse_update(body: &[u8]) -> Option<Update> {
    let length =
        usize::from_str_radix(core::str::from_utf8(body.get(..4)?).ok()?, 16)
            .ok()?;
    let command = core::str::from_utf8(body.get(4..length)?).ok()?;
    let mut words = command.split([' ', '\0', '\n']);
    let old = String::from(words.next()?);
    let new = String::from(words.next()?);
    let name = String::from(words.next()?);
    if body.get(length..length.checked_add(4)?)? != b"0000" {
        return None;
    }
    Some(Update {
        old,
        new,
        name,
        pack: body.get(length + 4..)?.to_vec(),
    })
}

#[must_use]
pub fn packet(data: &str) -> String {
    format!("{:04x}{data}", data.len() + 4)
}

#[must_use]
pub fn advertisement(oid: &str, http: bool) -> String {
    let mut result = String::new();
    if http {
        result.push_str(&packet("# service=git-receive-pack\n"));
        result.push_str("0000");
    }
    let name = if oid == ZERO { "capabilities^{}" } else { REF };
    result.push_str(&packet(&format!("{oid} {name}\0report-status\n")));
    result.push_str("0000");
    result
}

#[must_use]
pub fn report(rejection: Option<&str>) -> String {
    let mut result = packet("unpack ok\n");
    result.push_str(&packet(&rejection.map_or_else(
        || format!("ok {REF}\n"),
        |reason| format!("ng {REF} {reason}\n"),
    )));
    result.push_str("0000");
    result
}

#[must_use]
pub fn response(
    status: u16,
    content_type: &str,
    extra: &str,
    body: &[u8],
) -> Vec<u8> {
    if status == 0 {
        return Vec::new();
    }
    let mut result = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n", body.len()).into_bytes();
    result.extend_from_slice(body);
    result
}

#[must_use]
pub fn negotiation(
    expected: &str,
    candidate: &str,
    updates: &[(String, String, String)],
) -> bool {
    negotiation_refusal(expected, candidate, updates).is_none()
}

#[must_use]
pub fn negotiation_refusal(
    expected: &str,
    candidate: &str,
    updates: &[(String, String, String)],
) -> Option<&'static str> {
    let [(old, new, name)] = updates else {
        return Some("publication must negotiate exactly one ref");
    };
    if name != REF {
        return Some("unexpected destination ref");
    }
    if old != expected {
        return Some("advertised remote ref differs from expected state");
    }
    if new != candidate {
        return Some("publication differs from exact requested revision");
    }
    None
}

#[must_use]
pub fn accepted(
    transport: bool,
    statuses: &[(String, Option<String>)],
) -> bool {
    acceptance_refusal(transport, statuses).is_none()
}

#[must_use]
pub fn acceptance_refusal(
    transport: bool,
    statuses: &[(String, Option<String>)],
) -> Option<&'static str> {
    if !transport {
        return Some("transport did not complete");
    }
    let [(name, status)] = statuses else {
        return Some("expected exactly one ref status");
    };
    if name != REF {
        return Some("status belongs to another ref");
    }
    if status.is_some() {
        return Some("server refused the ref update");
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Credential {
    Username,
    Agent,
    Token,
    Refuse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Https,
    Ssh,
}

#[must_use]
pub const fn credential(
    transport: Transport,
    username: bool,
    agent: bool,
    token: bool,
    attempts: usize,
) -> Credential {
    if attempts > 2 {
        return Credential::Refuse;
    }
    if matches!(transport, Transport::Ssh) {
        if username {
            return Credential::Username;
        }
        if agent {
            return Credential::Agent;
        }
    } else if token {
        return Credential::Token;
    }
    Credential::Refuse
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Authentication,
    Access,
    RateLimited,
    Unavailable,
}

#[must_use]
pub const fn failure(
    status: u16,
    rate_remaining: Option<u64>,
    retry_after: bool,
) -> Failure {
    match status {
        401 => Failure::Authentication,
        403 | 429
            if matches!(rate_remaining, Some(0))
                || retry_after
                || status == 429 =>
        {
            Failure::RateLimited
        }
        403 | 404 => Failure::Access,
        _ => Failure::Unavailable,
    }
}

#[must_use]
pub const fn refusal(reject: bool, stale: bool) -> Option<&'static str> {
    match (reject, stale) {
        (true, _) => Some("fixture policy refused this ref"),
        (_, true) => Some("fixture expected old ref changed"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_complete_ref_evidence() {
        let good = [(String::from(REF), None)];
        assert!(accepted(true, &good));
        assert!(!accepted(false, &good));
        assert!(!accepted(true, &[]));
        assert!(!accepted(
            true,
            &[(String::from(REF), Some(String::from("denied")))]
        ));
        assert!(!accepted(true, &[(String::from("refs/heads/other"), None)]));
    }

    #[test]
    fn refuses_stale_remote_and_wrong_revision() {
        let updates =
            [(String::from("old"), String::from("new"), String::from(REF))];
        assert!(negotiation("old", "new", &updates));
        assert!(!negotiation("stale", "new", &updates));
        assert!(!negotiation("old", "other", &updates));
        assert!(!negotiation("old", "new", &[]));
    }

    #[test]
    fn authentication_and_rate_limit_are_distinct() {
        assert_eq!(failure(401, None, false), Failure::Authentication);
        assert_eq!(failure(403, None, false), Failure::Access);
        assert_eq!(failure(404, None, false), Failure::Access);
        assert_eq!(failure(403, Some(0), false), Failure::RateLimited);
        assert_eq!(failure(403, None, true), Failure::RateLimited);
        assert_eq!(failure(429, None, false), Failure::RateLimited);
        assert_eq!(failure(503, None, false), Failure::Unavailable);
    }

    #[test]
    fn credentials_refuse_unknown_types_and_bound_repeated_challenges() {
        assert_eq!(
            credential(Transport::Https, false, false, true, 1),
            Credential::Token
        );
        assert_eq!(
            credential(Transport::Ssh, false, true, false, 1),
            Credential::Agent
        );
        assert_eq!(
            credential(Transport::Ssh, true, false, false, 1),
            Credential::Username
        );
        assert_eq!(
            credential(Transport::Ssh, false, false, true, 1),
            Credential::Refuse
        );
        assert_eq!(
            credential(Transport::Https, false, false, true, 3),
            Credential::Refuse
        );
    }
}
