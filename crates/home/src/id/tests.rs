use super::{Draws, ProjectId};
use alloc::string::ToString;
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
