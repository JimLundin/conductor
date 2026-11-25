#![cfg(unix)]

mod support;

use std::path::PathBuf;
use std::time::Duration;

use agents::runtime::{
    Command, Directory, Event, Outcome, Session, Start, Started,
};
use agents::{ClientName, Decision, Effort, ModelId, Output, TurnOutcome};
use agents::{NativeRequest, Reply, Step};
use install::Home;
use runtime::{Instance, Launch, Runtime, Settings};

use support::{TestResult, committed_fake_cli_path, fake_codex_dir, fixture};

fn stored_gateway() -> agents::Credential {
    agents::Credential {
        kind: String::from("provider"),
        values: [
            ("name", "gateway"),
            ("base_url", "https://gateway.example"),
            ("key", "conductor-test-key"),
            ("model", "gateway-model"),
        ]
        .map(|(key, value)| (String::from(key), String::from(value)))
        .into(),
    }
}

#[tokio::test]
async fn configuring_and_clearing_a_codex_provider_changes_the_key_and_named_default()
-> TestResult<()> {
    let work = fake_codex_dir(
        &fixture("../e2e/codex-two-threads-listed.jsonl"),
        "configure",
    )?;
    let plugin = codex_cli::plugin(runtime::Settings::default());
    let instance = (plugin.instance)(runtime::Launch {
        path: committed_fake_cli_path(),
        home: Home::private(&work, "gateway", &plugin.profile.config)?,
        work: work.clone(),
    });
    let configured = instance.configure(Some(stored_gateway())).await?;
    assert_eq!(
        configured.named,
        Some(agents::ModelId(String::from("gateway-model")))
    );
    assert_eq!(
        configured.models?.default,
        Some(agents::ModelId(String::from("gpt-6.1-sol")))
    );
    assert!(
        std::fs::read_to_string(work.join("env.txt"))?
            .contains("CONDUCTOR_CODEX_KEY=conductor-test-key")
    );
    assert!(
        std::fs::read_to_string(work.join("argv.txt"))?
            .starts_with("run\n-c\nanalytics.enabled=false\napp-server\n")
    );
    let cleared = instance.configure(None).await?;
    assert_eq!(cleared.named, None);
    assert_eq!(
        cleared.models?.default,
        Some(agents::ModelId(String::from("gpt-6.1-sol")))
    );
    assert!(
        !std::fs::read_to_string(work.join("env.txt"))?
            .contains("CONDUCTOR_CODEX_KEY=")
    );
    assert!(
        !std::fs::read_to_string(work.join("codex-config.toml"))?
            .contains("gateway-model")
    );
    Ok(())
}

#[tokio::test]
async fn a_malformed_codex_provider_config_refuses_the_credential_before_discovery()
-> TestResult<()> {
    let work = fake_codex_dir(&fixture("server2-resume.jsonl"), "malformed")?
        .join("bad-config");
    let plugin = codex_cli::plugin(runtime::Settings::default());
    let home = Home::private(&work, "gateway", &plugin.profile.config)?;
    std::fs::write(work.join("home/gateway/config.toml"), "private-value = [")?;
    let instance = (plugin.instance)(runtime::Launch {
        path: committed_fake_cli_path(),
        home,
        work: work.clone(),
    });
    let reason = instance
        .configure(Some(stored_gateway()))
        .await
        .err()
        .ok_or("expected a credential refusal")?;
    assert!(reason.contains("config.toml"));
    assert!(!reason.contains("private-value"));
    std::fs::write(
        work.join("home/gateway/config.toml"),
        "model_providers = 1",
    )?;
    let conflict = instance
        .configure(Some(stored_gateway()))
        .await
        .err()
        .ok_or("expected a conflicting provider table to be refused")?;
    assert!(conflict.contains("model_providers"));
    Ok(())
}

fn instance(work: PathBuf) -> Instance {
    let plugin = codex_cli::plugin(Settings::default());
    (plugin.instance)(Launch {
        path: committed_fake_cli_path(),
        home: Home::User,
        work,
    })
}

async fn next_event(agent: &mut Runtime) -> TestResult<Event> {
    tokio::time::timeout(Duration::from_secs(20), agent.events.recv())
        .await?
        .ok_or_else(|| "the runtime event stream ended early".into())
}

async fn outcome(agent: &mut Runtime) -> TestResult<Outcome> {
    loop {
        if let Event::Outcome(result) = next_event(agent).await? {
            return Ok(result);
        }
    }
}

async fn start(
    agent: &mut Runtime,
    session: u128,
    conversation: &str,
) -> TestResult<()> {
    agent
        .commands
        .send(Command::Start(Start {
            session: Session(session),
            directory: Directory(String::from("/tmp/s2-scratch")),
            model: Some(ModelId(String::from("gpt-6-luna"))),
            effort: Some(Effort::Low),
            conversation: None,
            instructions: None,
        }))
        .await?;
    assert_eq!(
        outcome(agent).await?,
        Outcome::Started(Ok(Started {
            conversation: String::from(conversation),
        }))
    );
    Ok(())
}

async fn send(agent: &mut Runtime, text: &str) -> TestResult<()> {
    agent
        .commands
        .send(Command::Send {
            text: String::from(text),
            effort: None,
        })
        .await?;
    assert_eq!(outcome(agent).await?, Outcome::Sent(Ok(())));
    Ok(())
}

async fn approval(agent: &mut Runtime) -> TestResult<NativeRequest> {
    loop {
        if let Event::Step(Step::Open { native, .. }) =
            next_event(agent).await?
        {
            return Ok(native);
        }
    }
}

async fn reply(
    agent: &Runtime,
    native: NativeRequest,
    decision: Decision,
) -> TestResult<()> {
    agent
        .commands
        .send(Command::Reply {
            native,
            reply: Reply::Decide(decision),
            by: ClientName(String::from("tester")),
        })
        .await?;
    Ok(())
}

async fn messages(agent: &mut Runtime) -> TestResult<Vec<String>> {
    let mut messages = Vec::new();
    loop {
        match next_event(agent).await? {
            Event::Step(Step::Emit(Output::AssistantMessage { text })) => {
                messages.push(text);
            }
            Event::Step(Step::TurnEnded(result)) => {
                assert_eq!(result, TurnOutcome::Completed);
                return Ok(messages);
            }
            _ => {}
        }
    }
}

#[tokio::test]
async fn one_instance_routes_interleaved_turns_and_answers_each_approval_once()
-> TestResult<()> {
    let work = fake_codex_dir(
        &fixture("../e2e/codex-two-threads-listed.jsonl"),
        "sessions",
    )?;
    let instance = instance(work.clone());
    let _ = instance.configure(None).await?.models?;
    let mut first = instance.clone().start();
    let mut second = instance.start();
    start(&mut first, 1, "01a1cde1-361a-7dc2-a23c-72b2e1b8dcd8").await?;
    start(&mut second, 2, "01a1cde1-c749-7f40-8e2b-6b6df1bebf01").await?;

    send(&mut first, "create a.txt").await?;
    let first_request = approval(&mut first).await?;
    send(&mut second, "create b.txt").await?;
    let second_request = approval(&mut second).await?;
    assert_ne!(first_request, second_request);

    reply(&first, first_request.clone(), Decision::Approve).await?;
    assert_eq!(outcome(&mut first).await?, Outcome::Replied(Ok(())));
    reply(&second, second_request.clone(), Decision::Deny).await?;
    assert_eq!(outcome(&mut second).await?, Outcome::Replied(Ok(())));
    assert_eq!(
        messages(&mut first).await?,
        ["Created [a.txt](/tmp/s2-scratch/a.txt)."]
    );
    assert_eq!(messages(&mut second).await?, ["The request was declined."]);

    reply(&first, first_request, Decision::Approve).await?;
    assert!(matches!(
        outcome(&mut first).await?,
        Outcome::Replied(Err(_))
    ));
    reply(&second, second_request, Decision::Deny).await?;
    assert!(matches!(
        outcome(&mut second).await?,
        Outcome::Replied(Err(_))
    ));
    assert_eq!(std::fs::read_to_string(work.join("runs.txt"))?, "1");
    Ok(())
}
