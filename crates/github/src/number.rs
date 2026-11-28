use core::fmt;
use core::num::NonZeroU64;

use home::TrackerId;
use home::tracker::Issuer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IssueNumber(NonZeroU64);

impl IssueNumber {
    #[must_use]
    pub fn new(number: u64) -> Option<Self> {
        NonZeroU64::new(number).map(Self)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl TrackerId for IssueNumber {
    fn read(text: &str) -> Option<Self> {
        if text.starts_with('0')
            || !text.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        text.parse::<NonZeroU64>().ok().map(Self)
    }
}

impl fmt::Display for IssueNumber {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "{}", self.0)
    }
}

pub const ISSUER: Issuer = Issuer::of::<IssueNumber>(crate::ADAPTER);

#[cfg(test)]
mod tests {
    use super::{ISSUER, IssueNumber};
    use alloc::string::ToString;

    #[test]
    fn the_github_tracker_round_trips_its_own_issue_numbers() {
        let read = ISSUER.read("146");
        assert_eq!(
            read.as_ref().map(ToString::to_string).as_deref(),
            Some("146")
        );
        assert_eq!(
            read.and_then(|id| id.tracked::<IssueNumber>()),
            IssueNumber::new(146)
        );
    }

    #[test]
    fn the_github_tracker_refuses_a_non_numeric_id() {
        assert_eq!(
            ["team42/7", "0tm7aaaa", "", "0", "007", "+7", "7 "]
                .map(|text| ISSUER.read(text)),
            [None, None, None, None, None, None, None]
        );
    }
}
