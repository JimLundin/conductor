use alloc::string::String;

use proto::{Instance, Instances, Provider};

use super::*;

fn desired(version: u64, value: &str) -> Desired {
    Desired {
        version,
        revision: proto::fleet::Revision {
            pins: proto::fleet::Pins(BTreeMap::new()),
            settings: proto::fleet::Settings::default(),
        },
        credentials: Roster(
            [(
                instance(),
                Instance {
                    credential: Some(Credential {
                        kind: String::from("fixture"),
                        values: [(String::from("token"), String::from(value))]
                            .into(),
                    }),
                    ..Instance::default()
                },
            )]
            .into(),
        ),
        owned: BTreeSet::new(),
    }
}

fn instance() -> InstanceId {
    InstanceId::named(Provider::Claude, "fixture")
}

fn applied() -> CommandResult {
    CommandResult::Instances(Instances::default())
}

#[test]
fn save_failures_retry_with_a_capped_delay() {
    let mut core = Reconciliation::default();
    core.desire(desired(1, "first"));
    let work = core.plan(0).remove(0);
    let failed = CommandResult::Rejected(Rejection::CredentialNotSaved {
        reason: String::from("fixture disk failure"),
    });
    let mut now = 0;
    for delay in [1, 2, 4, 8, 16, 30, 30] {
        core.completed(work.clone(), &failed, now);
        assert_eq!(core.plan(now + delay - 1), []);
        now += delay;
        assert_eq!(core.plan(now), core::slice::from_ref(&work));
    }
    core.completed(work, &applied(), now);
    assert!(core.settled());
    assert_eq!(core.owned(), &BTreeSet::from([instance()]));
}

#[test]
fn refusal_waits_for_correction_without_restarting_for_an_unrelated_version() {
    let mut core = Reconciliation::default();
    core.desire(desired(1, "first"));
    let work = core.plan(0).remove(0);
    core.completed(
        work,
        &CommandResult::Rejected(Rejection::InvalidCredential {
            reason: String::from("fixture refusal"),
        }),
        0,
    );
    core.desire(desired(2, "first"));
    assert_eq!(core.plan(1000), []);
    assert!(!core.settled());
    core.desire(desired(3, "corrected"));
    let replacement = core.plan(1000).remove(0);
    core.completed(replacement, &applied(), 1000);
    core.desire(desired(4, "corrected"));
    assert_eq!(core.plan(1001), []);
    assert!(core.settled());
}

#[test]
fn stale_completion_and_older_desire_cannot_restore_superseded_credentials() {
    let mut core = Reconciliation::default();
    core.desire(desired(1, "old"));
    let stale = core.plan(0).remove(0);
    core.desire(desired(2, "new"));
    core.completed(stale, &applied(), 0);
    assert!(!core.settled());
    let latest = core.plan(0).remove(0);
    core.completed(latest, &applied(), 0);
    core.desire(desired(1, "old"));
    assert!(core.settled());
}

#[test]
fn removal_is_reconstructed_from_durable_ownership_after_restart() {
    let mut core = Reconciliation::default();
    let mut removed = desired(2, "unused");
    removed.credentials = Roster::default();
    removed.owned.insert(instance());
    core.desire(removed);
    let work = core.plan(0).remove(0);
    assert_eq!(work, Work::Remove(instance()));
    core.completed(
        work,
        &CommandResult::Rejected(Rejection::UnknownInstance),
        0,
    );
    assert!(core.settled());
    assert!(core.owned().is_empty());
}
