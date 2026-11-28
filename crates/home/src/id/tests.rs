use super::{Draws, IssueId, ProjectId, Slug, TrackerId};
use crate::tracker::Issuer;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

fn draws(given: &[u64]) -> Draws {
    core::array::from_fn(|at| {
        given.get(at).or_else(|| given.last()).copied().unwrap_or(0)
    })
}

#[test]
fn a_project_id_reads_back_as_it_is_written() {
    let id = ProjectId::mint(&draws(&[0x3fff_ffff]), &[]);
    assert_eq!(id.map(|minted| minted.to_string()), Some("zzzzzz".into()));
    assert_eq!(ProjectId::parse("a1b2c3").map(|id| id.to_string()), {
        Some("a1b2c3".to_string())
    });
}

#[test]
fn only_6_lower_case_base_32_digits_are_a_project_id() {
    assert_eq!(
        [
            "01m7r5n23kp17wyfj138e0bcnf",
            "a1b2c",
            "a1b2c3d",
            "A1B2C3",
            "a1b2l3"
        ]
        .map(ProjectId::parse),
        [None; 5]
    );
}

#[test]
fn minting_a_project_id_the_home_has_draws_again() {
    let held = Vec::from_iter(ProjectId::parse("000001"));
    assert_eq!(
        ProjectId::mint(&draws(&[1, 2]), &held),
        ProjectId::parse("000002")
    );
}

macro_rules! parses {
    ($id:ty, $($bound:tt)+) => {{
        struct Probe<T>(core::marker::PhantomData<T>);
        trait Unparsed {
            const PARSES: bool = false;
        }
        impl<T> Unparsed for Probe<T> {}
        impl<T: $($bound)+> Probe<T> {
            const PARSES: bool = true;
        }
        (<Probe<$id>>::PARSES, <Probe<String>>::PARSES)
    }};
}

#[test]
fn the_core_offers_no_parser_of_an_issue_id() {
    assert_eq!(parses!(IssueId, core::str::FromStr), (false, true));
    assert_eq!(parses!(IssueId, for<'a> From<&'a str>), (false, true));
    assert_eq!(parses!(IssueId, From<String>), (false, true));
    assert_eq!(parses!(IssueId, for<'a> TryFrom<&'a str>), (false, true));
    assert_eq!(parses!(IssueId, TryFrom<String>), (false, true));
}

#[test]
fn the_markdown_tracker_round_trips_its_own_slugs() {
    let read = Issuer::MARKDOWN.read("0tm7aaaa");
    assert_eq!(
        read.as_ref().map(ToString::to_string).as_deref(),
        Some("0tm7aaaa")
    );
    assert_eq!(
        read.and_then(|id| id.tracked::<Slug>()),
        Slug::read("0tm7aaaa")
    );
    assert_eq!(
        ["146", "team42/7", "0TM7AAAA", "0tm7aaa"]
            .map(|text| Issuer::MARKDOWN.read(text)),
        [None, None, None, None]
    );
}
