use home::issue::{EditId, IssueWrite};
use home::operations::{Handle, Intent, Work};
use project_runtime::operations::{Error, Operations};

fn changes() -> Result<Intent, &'static str> {
    Ok(Intent {
        project: home::ProjectId::new(1),
        expected: String::from("issue-1"),
        work: Work::ChangeIssue {
            issue: github::ISSUER.read("42").ok_or("bad fixture")?,
            writes: Vec::from([IssueWrite::Title {
                from: String::from("Draft"),
                to: String::from("Ready"),
            }]),
        },
    })
}

#[test]
fn a_retry_on_another_host_cannot_start_even_with_no_local_record()
-> Result<(), Box<dyn std::error::Error>> {
    let data = tempfile::tempdir()?;
    let local = Operations::bind(data.path(), [2; 32])?;
    let foreign = Handle {
        owner: [1; 32],
        id: EditId(3),
    };
    assert!(matches!(
        local.begin(foreign, changes()?, true),
        Err(Error::Refused(home::operations::Refusal::OwnerUnavailable))
    ));
    assert!(matches!(
        local.status(foreign),
        Err(Error::Refused(home::operations::Refusal::OwnerUnavailable))
    ));
    Ok(())
}

fn transition() -> Result<Intent, &'static str> {
    let mut intent = changes()?;
    let Work::ChangeIssue { writes, .. } = &mut intent.work else {
        return Err("not an issue change");
    };
    *writes = Vec::from([
        IssueWrite::RemoveLabel(String::from("work/status/open")),
        IssueWrite::AddLabel(String::from("work/status/active")),
    ]);
    Ok(intent)
}

fn evidence(step: Step, revision: &str) -> Confirmed {
    Confirmed {
        step,
        revision: String::from(revision),
        identity: Some(String::from("receipt-42")),
    }
}

use home::operations::{Confirmed, Progress, Refusal, Step};

#[test]
fn a_partially_applied_transition_resumes_only_against_its_verified_state()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let daemon = Operations::bind(dir.path(), [1; 32])?;
    let cli = Operations::open(dir.path())?;
    let handle = daemon.handle()?;
    let change = transition()?;
    let removed = Step::WriteIssue(0);
    let added = Step::WriteIssue(1);
    let mut attempt = daemon.begin(handle, change.clone(), true)?;
    attempt.dispatch(removed.clone(), "issue-1")?;
    assert!(matches!(
        cli.begin(handle, change.clone(), true),
        Err(Error::InProgress)
    ));
    attempt.confirm(evidence(removed.clone(), "old-label-removed"), false)?;
    drop(attempt);
    let mut retry = cli.begin(handle, change.clone(), true)?;
    assert_eq!(
        retry.record().confirmed(),
        [evidence(removed, "old-label-removed")]
    );
    assert!(matches!(
        retry.dispatch(added.clone(), "human-edit"),
        Err(Error::Refused(Refusal::Conflict))
    ));
    retry.dispatch(added.clone(), "old-label-removed")?;
    drop(retry);
    let mut reconciled = daemon.begin(handle, change.clone(), true)?;
    assert_eq!(
        reconciled.record().progress(),
        &Progress::AwaitingEvidence(added.clone())
    );
    assert!(matches!(
        reconciled.dispatch(added.clone(), "old-label-removed"),
        Err(Error::Refused(Refusal::AwaitingEvidence))
    ));
    reconciled.confirm(evidence(added, "verified-active"), true)?;
    let success = reconciled.record().clone();
    drop(reconciled);
    let offline = Operations::open(dir.path())?;
    assert_eq!(offline.status(handle)?, Some(success.clone()));
    assert_eq!(offline.begin(handle, change, false)?.record(), &success);
    assert!(matches!(
        offline.begin(handle, changes()?, true),
        Err(Error::Refused(Refusal::ChangedIntent))
    ));
    Ok(())
}

#[test]
fn an_offline_request_creates_no_record_and_cannot_dispatch_a_paused_edit()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Operations::bind(dir.path(), [7; 32])?;
    let handle = store.handle()?;
    let input = changes()?;
    assert!(matches!(
        store.begin(handle, input.clone(), false),
        Err(Error::Refused(Refusal::Offline))
    ));
    assert_eq!(store.status(handle)?, None);
    drop(store.begin(handle, input.clone(), true)?);
    let mut retry = store.begin(handle, input, false)?;
    assert!(matches!(
        retry.dispatch(Step::WriteIssue(0), "issue-1"),
        Err(Error::Refused(Refusal::Offline))
    ));
    Ok(())
}

#[test]
fn rebinding_a_host_data_directory_cannot_adopt_its_records()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Operations::bind(dir.path(), [7; 32])?;
    let handle = store.handle()?;
    drop(store.begin(handle, changes()?, true)?);
    assert!(matches!(
        Operations::bind(dir.path(), [8; 32]),
        Err(Error::WrongHost)
    ));
    assert_eq!(
        Operations::open(dir.path())?
            .status(handle)?
            .map(|r| r.handle),
        Some(handle)
    );
    Ok(())
}

#[test]
fn an_interrupted_attempt_can_be_inspected_before_the_host_restarts()
-> Result<(), Box<dyn std::error::Error>> {
    const CHILD: &str = "PROJECT_OPERATION_INTERRUPT_FIXTURE";
    let owner = [3; 32];
    let handle = Handle {
        owner,
        id: EditId(20),
    };
    if let Some(data) = std::env::var_os(CHILD) {
        let store = Operations::open(std::path::Path::new(&data))?;
        let mut attempt = store.begin(handle, changes()?, true)?;
        attempt.dispatch(Step::WriteIssue(0), "issue-1")?;
        std::process::abort();
    }
    let dir = tempfile::tempdir()?;
    let _daemon = Operations::bind(dir.path(), owner)?;
    let child = std::process::Command::new(std::env::current_exe()?)
        .arg("--exact")
        .arg("an_interrupted_attempt_can_be_inspected_before_the_host_restarts")
        .env(CHILD, dir.path())
        .current_dir(dir.path())
        .output()?;
    assert!(!child.status.success());
    let reopened = Operations::open(dir.path())?;
    let status = reopened.status(handle)?.ok_or("attempt disappeared")?;
    assert_eq!(
        status.progress(),
        &Progress::AwaitingEvidence(Step::WriteIssue(0))
    );
    assert_eq!(reopened.begin(handle, changes()?, true)?.record(), &status);
    Ok(())
}

#[test]
fn a_write_whose_request_is_out_lets_a_write_on_another_handle_begin()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let daemon = Operations::bind(dir.path(), [4; 32])?;
    let cli = Operations::open(dir.path())?;
    let mut out = daemon.begin(daemon.handle()?, changes()?, true)?;
    out.dispatch(Step::WriteIssue(0), "issue-1")?;
    let other = cli.handle()?;
    let mut second = cli.begin(other, changes()?, true)?;
    second.dispatch(Step::WriteIssue(0), "issue-1")?;
    out.confirm(evidence(Step::WriteIssue(0), "issue-2"), true)?;
    assert_eq!(
        daemon
            .status(other)?
            .map(|record| record.progress().clone()),
        Some(Progress::AwaitingEvidence(Step::WriteIssue(0)))
    );
    Ok(())
}

#[test]
fn a_second_write_on_a_handle_whose_request_is_out_is_refused_as_in_progress()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let daemon = Operations::bind(dir.path(), [5; 32])?;
    let cli = Operations::open(dir.path())?;
    let handle = daemon.handle()?;
    let mut out = daemon.begin(handle, changes()?, true)?;
    out.dispatch(Step::WriteIssue(0), "issue-1")?;
    assert!(matches!(
        cli.begin(handle, changes()?, true),
        Err(Error::InProgress)
    ));
    assert!(matches!(
        daemon.begin(handle, changes()?, true),
        Err(Error::InProgress)
    ));
    drop(out);
    assert_eq!(
        cli.begin(handle, changes()?, true)?.record().progress(),
        &Progress::AwaitingEvidence(Step::WriteIssue(0))
    );
    Ok(())
}
