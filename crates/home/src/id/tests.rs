use alloc::string::ToString;
use alloc::vec::Vec;

use super::{DRAWS, Draws, IssueId, ProjectId};

/// 2026-11-22T12:00:00Z.
const NOW: u64 = 1_795_348_800;

fn minted(now: u64, random: u64) -> Option<IssueId> {
    IssueId::mint(now, &[random; DRAWS], &[])
}

/// The draws `given`, the last of them repeated to the end.
fn draws(given: &[u64]) -> Draws {
    core::array::from_fn(|at| {
        given.get(at).or_else(|| given.last()).copied().unwrap_or(0)
    })
}

#[test]
fn an_issue_id_writes_its_seconds_since_2026_then_its_draw() {
    assert_eq!(
        [
            minted(IssueId::EPOCH, 0),
            minted(IssueId::EPOCH + 1, 31),
            minted(NOW, 7),
        ]
        .map(|id| id.map(|minted| minted.to_string())),
        [
            Some("00000000".to_string()),
            Some("0000010z".to_string()),
            Some("0tt82007".to_string()),
        ]
    );
}

#[test]
fn issue_ids_sort_by_when_they_were_minted() {
    let ids: Vec<IssueId> = [(NOW, 1023), (NOW + 1, 0), (NOW + 3600, 5)]
        .into_iter()
        .filter_map(|(now, random)| minted(now, random))
        .collect();
    let texts: Vec<_> = ids.iter().map(ToString::to_string).collect();
    assert!(ids.is_sorted() && texts.is_sorted() && ids.len() == 3);
}

#[test]
fn minting_an_id_the_home_has_draws_again() {
    let held = Vec::from_iter(minted(NOW, 7));
    assert_eq!(
        IssueId::mint(NOW, &draws(&[7, 7, 9]), &held),
        minted(NOW, 9)
    );
    assert_eq!(IssueId::mint(NOW, &draws(&[7]), &held), None);
}

#[test]
fn no_issue_id_counts_a_time_outside_its_years() {
    assert_eq!(
        [IssueId::EPOCH - 1, 2_840_967_424].map(|now| minted(now, 0)),
        [None, None]
    );
    assert!(minted(2_840_967_423, 0).is_some());
}

#[test]
fn an_issue_id_reads_back_as_it_is_written() {
    let id = minted(NOW, 7);
    assert_eq!(
        id.and_then(|minted| IssueId::parse(&minted.to_string())),
        id
    );
    assert_eq!(IssueId::parse("zzzzzzzz").map(|id| id.to_string()), {
        Some("zzzzzzzz".to_string())
    });
}

#[test]
fn only_8_lower_case_base_32_digits_are_an_issue_id() {
    assert_eq!(
        [
            "01m82c165wagy4ctje7xncgje4",
            "0tt8200",
            "0tt820070",
            "0TT82007",
            "0tt8200i",
            "0tt8200u",
            "",
        ]
        .map(IssueId::parse),
        [None; 7]
    );
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

#[test]
fn an_id_crosses_the_wire_as_its_text_and_nothing_else() {
    let id = minted(NOW, 7);
    let bytes = postcard::to_allocvec(&id).unwrap_or_default();
    assert_eq!(
        postcard::from_bytes::<Option<IssueId>>(&bytes).ok(),
        Some(id)
    );
    let foreign = postcard::to_allocvec(&Some("01m82c165wagy4ctje7xncgje4"))
        .unwrap_or_default();
    assert!(postcard::from_bytes::<Option<IssueId>>(&foreign).is_err());
}

#[test]
fn an_id_keeps_of_a_draw_only_the_bits_it_has_room_for() {
    assert_eq!(
        minted(IssueId::EPOCH, u64::MAX).map(|id| id.to_string()),
        Some("000000zz".to_string())
    );
    assert_eq!(
        ProjectId::mint(&[u64::MAX; DRAWS], &[]).map(|id| id.to_string()),
        Some("zzzzzz".to_string())
    );
}
