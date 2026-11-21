use agents::{Failure, Invocation, Pick, SignIn as Flow, Word};
use proto::{
    Account, CommandResult, Instance, InstanceId, Provider, Rejection, SignIn,
    SignInCode, SignInStep,
};

use super::{Effect, Key, Login, Run, SignIns, release};
use crate::registry::AccountAdoption;

fn id() -> InstanceId {
    InstanceId::own(Provider::ALL[0])
}

fn settings() -> Instance {
    Instance::default()
}

fn key(run: u64) -> Key {
    Key {
        instance: id(),
        run: Run(run),
    }
}

/// A sign-in whose page gives a code to paste back.
fn pasted() -> Flow {
    Flow {
        run: Invocation::default(),
        link: Pick {
            after: String::from("visit:"),
            word: Word::Link,
        },
        code: None,
        paste: true,
        failures: Vec::from([Failure {
            says: String::from("wasn't accepted"),
            means: String::from("That code was refused."),
        }]),
    }
}

/// A sign-in by a code the person enters on the page.
fn device() -> Flow {
    Flow {
        code: Some(Pick {
            after: String::from("code:"),
            word: Word::Code,
        }),
        paste: false,
        ..pasted()
    }
}

fn step(step: SignInStep) -> Effect {
    Effect::Publish(SignIn {
        instance: id(),
        step,
    })
}

fn open(code: Option<&str>, paste: bool) -> SignInStep {
    SignInStep::Open {
        url: String::from("https://sign.example/in?state=1"),
        code: code.map(String::from),
        paste,
    }
}

fn failed(reason: &str) -> Effect {
    step(SignInStep::Failed {
        reason: String::from(reason),
    })
}

fn code() -> SignInCode {
    SignInCode(String::from("pasted#code"))
}

/// A sign-in running `flow` as run 1, its link printed.
fn shown(flow: Flow) -> SignIns {
    let mut sign_ins = SignIns::default();
    let _ = sign_ins.start(id(), Some(settings()));
    let _ = sign_ins.spawned(&key(1), Ok(flow));
    let _ = sign_ins.printed(&key(1), "\u{1b}[1mOpening the browser\u{1b}[0m");
    let _ = sign_ins.printed(
        &key(1),
        "If it didn't open, visit: https://sign.example/in?state=1",
    );
    let _ = sign_ins.printed(&key(1), "code:");
    let _ = sign_ins.printed(&key(1), "  WXYZ-1234");
    sign_ins
}

#[test]
fn only_a_matching_retry_waits_for_the_recovered_account() {
    let mut sign_ins = SignIns::default();
    let adoption = AccountAdoption {
        from: id(),
        to: InstanceId::named(id().provider, "signed-in"),
    };
    assert_eq!(sign_ins.resume(&adoption.to, &adoption), Ok(None));
    assert_eq!(sign_ins.running(), []);
    let key = key(1);
    assert_eq!(
        sign_ins.resume(&adoption.from, &adoption),
        Ok(Some(key.clone()))
    );
    assert_eq!(
        release(&SignIn {
            instance: id(),
            step: SignInStep::Finishing
        }),
        None,
    );
    let done = SignIn {
        instance: id(),
        step: SignInStep::SignedIn,
    };
    assert_eq!(
        sign_ins.listed(&key, Some(CommandResult::Accepted)),
        Vec::from([Effect::Publish(done.clone())]),
    );
    assert_eq!(release(&done), Some(&adoption.from));
}

#[test]
fn a_pasted_code_goes_to_the_cli_and_a_checked_home_signs_the_instance_in() {
    let mut sign_ins = SignIns::default();
    let started = sign_ins.start(id(), Some(settings()));
    let again = sign_ins.start(id(), Some(settings()));
    let _ = sign_ins.spawned(&key(1), Ok(pasted()));
    let before = sign_ins.printed(&key(1), "Opening browser to sign in…");
    let link = sign_ins.printed(
        &key(1),
        "If the browser didn't open, visit: https://sign.example/in?state=1",
    );
    let later = sign_ins.printed(&key(1), "visit: https://another.example/");
    let snapshot = sign_ins.running();
    let submitted = sign_ins.submit(id(), code());
    let twice = sign_ins.submit(id(), code());
    let exited = sign_ins.exited(&key(1), Some(0), "exit status: 0");
    let checked = sign_ins.checked(
        &key(1),
        Ok(Some(Account::Named(String::from("me@example.com")))),
    );
    let listed = sign_ins.listed(&key(1), Some(CommandResult::Accepted));
    assert_eq!(
        (started, again, before, link, later, snapshot),
        (
            (
                CommandResult::Accepted,
                Vec::from([
                    Effect::Spawn {
                        key: key(1),
                        settings: settings(),
                    },
                    step(SignInStep::Starting),
                ])
            ),
            (
                CommandResult::Rejected(Rejection::SignInRunning),
                Vec::new()
            ),
            Vec::new(),
            Vec::from([step(open(None, true))]),
            Vec::new(),
            Vec::from([SignIn {
                instance: id(),
                step: open(None, true),
            }]),
        )
    );
    assert_eq!(
        (
            submitted,
            twice,
            exited,
            checked,
            listed,
            sign_ins.running()
        ),
        (
            (
                CommandResult::Accepted,
                Vec::from([
                    Effect::Write(key(1), code()),
                    step(SignInStep::Finishing),
                ])
            ),
            (CommandResult::Rejected(Rejection::NoCodeAsked), Vec::new()),
            Vec::from([
                Effect::Check(key(1), Login(())),
                step(SignInStep::Finishing)
            ]),
            Vec::from([Effect::Refresh(
                key(1),
                Some(Account::Named(String::from("me@example.com")))
            )]),
            Vec::from([step(SignInStep::SignedIn)]),
            Vec::new(),
        )
    );
}

#[test]
fn a_device_code_is_shown_with_its_link_and_no_code_is_taken() {
    let mut sign_ins = shown(device());
    assert_eq!(
        (
            sign_ins.running(),
            sign_ins.submit(id(), code()),
            sign_ins.exited(&key(1), Some(0), "exit status: 0"),
        ),
        (
            Vec::from([SignIn {
                instance: id(),
                step: open(Some("WXYZ-1234"), false),
            }]),
            (CommandResult::Rejected(Rejection::NoCodeAsked), Vec::new()),
            Vec::from([
                Effect::Check(key(1), Login(())),
                step(SignInStep::Finishing)
            ]),
        )
    );
}

#[test]
fn a_code_is_taken_only_once_the_page_can_have_given_one() {
    let mut sign_ins = SignIns::default();
    let unknown = sign_ins.start(id(), None);
    let none = sign_ins.submit(id(), code());
    let _ = sign_ins.start(id(), Some(settings()));
    let early = sign_ins.submit(id(), code());
    assert_eq!(
        (unknown, none, early),
        (
            (
                CommandResult::Rejected(Rejection::UnknownInstance),
                Vec::new()
            ),
            (CommandResult::Rejected(Rejection::NoCodeAsked), Vec::new()),
            (CommandResult::Rejected(Rejection::NoCodeAsked), Vec::new()),
        )
    );
}

#[test]
fn a_failure_says_what_the_cli_said_it_means_or_how_it_exited() {
    let mut refused = shown(pasted());
    let _ = refused.submit(id(), code());
    let _ = refused.printed(&key(1), "Login failed: that code wasn't accepted");
    let mut exited = shown(pasted());
    assert_eq!(
        (
            refused.exited(&key(1), Some(1), "exit status: 1"),
            exited.exited(&key(1), None, "signal: 9 (SIGKILL)"),
            refused.running(),
            exited.running(),
        ),
        (
            Vec::from([failed("That code was refused.")]),
            Vec::from([failed("the sign-in exited with signal: 9 (SIGKILL)")]),
            Vec::new(),
            Vec::new(),
        )
    );
}

#[test]
fn a_cancelled_run_is_killed_and_its_later_reports_are_dropped() {
    let mut sign_ins = shown(pasted());
    let cancelled = sign_ins.cancel(id());
    let nothing = sign_ins.cancel(id());
    let restarted = sign_ins.start(id(), Some(settings())).0;
    assert_eq!(
        (
            cancelled,
            nothing,
            restarted,
            sign_ins.printed(&key(1), "visit: https://sign.example/in?state=1"),
            sign_ins.exited(&key(1), Some(0), "exit status: 0"),
            sign_ins.checked(
                &key(1),
                Ok(Some(Account::Named(String::from("me@example.com")))),
            ),
            sign_ins.listed(&key(1), Some(CommandResult::Accepted)),
            sign_ins.expired(&key(1)),
            sign_ins.spawned(&key(1), Err(String::from("gone"))),
            sign_ins.running(),
        ),
        (
            (
                CommandResult::Accepted,
                Vec::from([Effect::Kill(key(1)), step(SignInStep::Cancelled)])
            ),
            (CommandResult::Accepted, Vec::new()),
            CommandResult::Accepted,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::from([SignIn {
                instance: id(),
                step: SignInStep::Starting,
            }]),
        )
    );
}

#[test]
fn a_run_that_doesnt_start_runs_too_long_or_leaves_the_home_signed_out_fails() {
    let mut unstarted = SignIns::default();
    let _ = unstarted.start(id(), Some(settings()));
    let mut slow = shown(device());
    let mut unchecked = shown(device());
    let _ = unchecked.exited(&key(1), Some(0), "exit status: 0");
    let mut unlisted = shown(device());
    assert_eq!(
        [
            unstarted.spawned(&key(1), Err(String::from("no such file"))),
            slow.expired(&key(1)),
            unchecked.checked(
                &key(1),
                Err(String::from(
                    "the sign-in ended, but the CLI's sign-in check still fails",
                )),
            ),
            unlisted.listed(&key(1), None),
        ],
        [
            Vec::from([failed("no such file")]),
            Vec::from([
                Effect::Kill(key(1)),
                failed("the sign-in took longer than 15 minutes"),
            ]),
            Vec::from([failed(
                "the sign-in ended, but the CLI's sign-in check still fails"
            )]),
            Vec::from([failed(
                "the host could not refresh the instance: None"
            )]),
        ]
    );
}

#[test]
fn a_step_is_logged_without_its_link_or_code() {
    let logged = [
        SignInStep::Starting,
        open(Some("WXYZ-1234"), true),
        SignInStep::Finishing,
        SignInStep::SignedIn,
        SignInStep::Failed {
            reason: String::from("refused"),
        },
        SignInStep::Cancelled,
    ]
    .map(|step| {
        super::note(&SignIn {
            instance: id(),
            step,
        })
    });
    assert_eq!(
        logged,
        [
            "starting",
            "open",
            "finishing",
            "signed in",
            "failed: refused",
            "cancelled"
        ]
        .map(|step| crate::Note::SignIn {
            instance: id(),
            step: String::from(step),
        })
    );
}

#[test]
fn output_before_the_link_is_kept_only_so_far() {
    let mut sign_ins = SignIns::default();
    let _ = sign_ins.start(id(), Some(settings()));
    let _ = sign_ins.spawned(&key(1), Ok(pasted()));
    let _ = sign_ins.printed(&key(1), "visit:");
    for _ in 0..super::KEPT {
        let _ = sign_ins.printed(&key(1), "noise");
    }
    assert_eq!(
        sign_ins.printed(&key(1), "https://sign.example/in?state=1"),
        Vec::new()
    );
}

#[test]
fn a_sign_in_succeeds_only_when_it_exits_zero() {
    assert_eq!(
        [
            Login::exited(Some(0), "exit status: 0"),
            Login::exited(Some(1), "exit status: 1"),
            Login::exited(None, "signal: 9"),
        ],
        [
            Ok(Login(())),
            Err(String::from("the sign-in exited with exit status: 1")),
            Err(String::from("the sign-in exited with signal: 9")),
        ]
    );
}
