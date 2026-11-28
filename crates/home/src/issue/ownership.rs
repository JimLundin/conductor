use crate::IssueId;
use crate::prelude::*;
use serde::{Deserialize, Serialize};
use vcs::Repository;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owner {
    pub repository: Repository,
    pub issue: IssueId,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reservation {
    Record(Owner),
    Existing(Owner),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conflict {
    OtherOwner(Owner),
    UnrecordedBranch,
    MultipleBranches,
}

pub fn reserve(
    repository: &Repository,
    issue: &IssueId,
    branch: &str,
    exists: bool,
    recorded: &[Owner],
) -> Result<Reservation, Conflict> {
    let owned: Vec<&Owner> = recorded
        .iter()
        .filter(|owner| {
            &owner.repository == repository && &owner.issue == issue
        })
        .collect();
    if owned.len() > 1 {
        return Err(Conflict::MultipleBranches);
    }
    let chosen = owned.first().map_or(branch, |owner| owner.branch.as_str());
    if let Some(other) = recorded.iter().find(|owner| {
        &owner.repository == repository
            && owner.branch == chosen
            && &owner.issue != issue
    }) {
        return Err(Conflict::OtherOwner(other.clone()));
    }
    if let Some(owner) = owned.first() {
        return Ok(Reservation::Existing((*owner).clone()));
    }
    if exists {
        return Err(Conflict::UnrecordedBranch);
    }
    Ok(Reservation::Record(Owner {
        repository: repository.clone(),
        issue: issue.clone(),
        branch: String::from(branch),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(text: &str) -> IssueId {
        IssueId(String::from(text))
    }

    #[test]
    fn an_issue_id_that_is_a_ref_name_names_its_branch() {
        assert_eq!(
            crate::flow::graph::branch_of(&id("146"), None),
            "issue/146"
        );
    }

    #[test]
    fn an_issue_id_that_is_no_ref_name_has_a_bounded_digest_as_its_branch() {
        let identity = id(&"https://tracker/~".repeat(30));
        let branch = crate::flow::graph::branch_of(&identity, None);
        assert_eq!(branch.len(), 70);
        assert!(branch.starts_with("issue/"));
        assert!(branch[6..].bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(branch, crate::flow::graph::branch_of(&id("other/1"), None));
    }

    #[test]
    fn another_issue_cannot_claim_a_recorded_branch_in_the_same_repository() {
        let owner = Owner {
            repository: Repository(String::from("repo")),
            issue: id("rest-1"),
            branch: String::from("shared"),
        };
        assert_eq!(
            reserve(
                &owner.repository,
                &id("rest-2"),
                "shared",
                false,
                core::slice::from_ref(&owner)
            ),
            Err(Conflict::OtherOwner(owner))
        );
    }

    #[test]
    fn an_existing_owners_choice_survives_a_changed_proposed_branch() {
        let owner = Owner {
            repository: Repository(String::from("repo")),
            issue: id("rest-1"),
            branch: String::from("kept"),
        };
        assert_eq!(
            reserve(
                &owner.repository,
                &owner.issue,
                "changed",
                true,
                core::slice::from_ref(&owner)
            ),
            Ok(Reservation::Existing(owner))
        );
    }
}

#[cfg(test)]
mod scope_tests {
    use super::*;

    #[test]
    fn identical_branch_names_can_be_owned_in_different_repositories() {
        let issue = IssueId(String::from("rest-42"));
        let owner = Owner {
            repository: Repository(String::from("repo-one")),
            issue: issue.clone(),
            branch: String::from("chosen"),
        };
        assert!(matches!(
            reserve(
                &Repository(String::from("repo-two")),
                &issue,
                "chosen",
                false,
                &[owner]
            ),
            Ok(Reservation::Record(_))
        ));
    }

    #[test]
    fn a_branch_without_an_owner_record_requires_reconciliation() {
        let issue = IssueId(String::from("rest-42"));
        assert_eq!(
            reserve(
                &Repository(String::from("repo")),
                &issue,
                "chosen",
                true,
                &[]
            ),
            Err(Conflict::UnrecordedBranch)
        );
    }
}

impl Reservation {
    #[must_use]
    pub const fn owner(&self) -> &Owner {
        match self {
            Self::Record(owner) | Self::Existing(owner) => owner,
        }
    }

    pub fn requested(&self, branch: &str) -> Result<(), String> {
        if self.owner().branch == branch {
            Ok(())
        } else {
            Err(format!(
                "this issue owns {}; its branch cannot be renamed by a refreshed issue",
                self.owner().branch
            ))
        }
    }
}
