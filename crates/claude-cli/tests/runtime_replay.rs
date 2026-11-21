#![cfg(test)]
#![cfg(unix)]

use std::path::Path;
use std::time::Duration;

use agents::runtime::{Command, Event, Outcome, Start};
use agents::{Reply, Step};
use install::Home;
use proto::{
    ClientName, Decision, Effort, EventKind, HostPath, ModelId, SessionId,
    TurnOutcome,
};
use runtime::{Instance, Launch, Runtime, Settings};

mod support;

use support::{TestResult, fake_claude, fake_cli_path, fixture};

fn instance(dir: &Path) -> Instance {
    let plugin = claude_cli::plugin(Settings::default());
    (plugin.instance)(Launch {
        path: fake_cli_path(),
        home: Home::User,
        work: dir.to_path_buf(),
        fleet_flags: None,
    })
}

#[tokio::test]
async fn configuring_a_claude_gateway_lists_its_models_and_efforts()
-> TestResult<()> {
    let path = fixture("session4-chosen-killed.jsonl");
    let fake = fake_claude("models", &path)?;
    let instance = instance(&fake);
    let credential = proto::Credential {
        kind: String::from("gateway"),
        values: [
            ("base_url", "https://gateway.example"),
            ("token", "conductor-test-token"),
        ]
        .map(|(key, value)| (String::from(key), String::from(value)))
        .into(),
    };
    let configured = instance.configure(Some(credential)).await?;
    let models = configured.models?;
    assert_eq!(configured.named, None);
    assert_eq!(
        (
            models
                .choices
                .iter()
                .map(|choice| (choice.id.0.as_str(), choice.name.as_str()))
                .collect::<Vec<(&str, &str)>>(),
            &models.default,
            models
                .find(&ModelId(String::from("haiku")))
                .map(|haiku| &haiku.efforts),
        ),
        (
            Vec::from([
                ("opus", "Opus"),
                ("claude-fable-5[1m]", "Fable"),
                ("sonnet", "Sonnet"),
                ("haiku", "Haiku"),
            ]),
            &Some(ModelId(String::from("opus"))),
            Some(&Vec::from([
                Effort::Low,
                Effort::Medium,
                Effort::High,
                Effort::XHigh,
                Effort::Max
            ])),
        )
    );
    Ok(())
}

async fn next_event(runtime: &mut Runtime) -> TestResult<Event> {
    tokio::time::timeout(Duration::from_secs(20), runtime.events.recv())
        .await?
        .ok_or_else(|| "runtime event stream ended".into())
}

#[tokio::test]
async fn a_session_approves_a_tool_and_observes_the_completed_turn()
-> TestResult<()> {
    let recording = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/e2e/claude-approve.jsonl");
    let fake = fake_claude("runtime-approve", &recording)?;
    let mut runtime = instance(&fake).start();
    runtime
        .commands
        .send(Command::Start(Start {
            session: SessionId(1),
            directory: HostPath(fake.to_string_lossy().into_owned()),
            model: None,
            effort: None,
            history: None,
            conversation: None,
            instructions: None,
        }))
        .await?;
    assert!(matches!(
        next_event(&mut runtime).await?,
        Event::Outcome(Outcome::Started(Ok(_)))
    ));

    runtime
        .commands
        .send(Command::Send {
            text: String::from("a.txt"),
            effort: None,
        })
        .await?;
    let request = loop {
        match next_event(&mut runtime).await? {
            Event::Step(Step::Open { native, .. }) => break native,
            Event::Outcome(Outcome::Sent(Err(error))) => {
                return Err(error.into());
            }
            _ => {}
        }
    };
    runtime
        .commands
        .send(Command::Reply {
            native: request,
            reply: Reply::Decide(Decision::Approve),
            by: ClientName(String::from("test")),
        })
        .await?;
    assert_eq!(
        next_event(&mut runtime).await?,
        Event::Outcome(Outcome::Replied(Ok(())))
    );
    let text = loop {
        if let Event::Step(Step::Emit(EventKind::AssistantMessage {
            text,
            ..
        })) = next_event(&mut runtime).await?
        {
            break text;
        }
    };
    assert_eq!(
        text,
        "I created `a.txt` in `/tmp/s1-scratch` with the content `hello from s1`."
    );
    loop {
        if let Event::Step(Step::TurnEnded(outcome)) =
            next_event(&mut runtime).await?
        {
            assert_eq!(outcome, TurnOutcome::Completed);
            break;
        }
    }
    runtime.commands.send(Command::Shutdown).await?;
    assert_eq!(
        next_event(&mut runtime).await?,
        Event::Outcome(Outcome::ShutDown(Ok(())))
    );
    Ok(())
}
