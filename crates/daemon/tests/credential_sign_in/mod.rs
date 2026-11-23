use std::path::Path;
use std::sync::Arc;

use proto::{
    Auth, Availability, CommandBody, CommandResult, Credential, Feed,
    InstanceId, Instances, Provider, Rejection, SignIn, SignInStep,
};
use runtime::Settings;
use tokio::sync::{mpsc, oneshot};

use super::{
    Fallible, capture_log, captured, credential, data_dir, disk, endpoint,
    fake_claude, holding, instance, probed, raw_history, serve, sign_in_until,
};
use crate::raw::{RawClient, Recorder};

const LABEL: &str = "paired fake setup";
const URL: &str =
    "https://example.invalid/authorize?state=credential-sign-in-test";
const CODE: &str = "fake-one-time-authorization-code";
const TOKEN: &str = "fake-minted-host-only-value";

#[derive(Debug)]
struct Setup(runtime::Launch);

impl runtime::ProviderInstance for Setup {
    fn configure(&self, given: Option<Credential>) -> runtime::Configuring<'_> {
        Box::pin(async move {
            if given
                .as_ref()
                .is_some_and(|credential| credential.value("token") != TOKEN)
            {
                return Err(String::from("unexpected test credential"));
            }
            Ok(agents::Configuration {
                named: None,
                models: Ok(probed()),
            })
        })
    }

    fn start(self: Arc<Self>) -> runtime::Runtime {
        let (commands, _) = mpsc::channel(1);
        let (_, events) = mpsc::channel(1);
        runtime::Runtime { commands, events }
    }

    fn credential_sign_in(
        &self,
        kind: &str,
        label: String,
        show: oneshot::Sender<String>,
        code: oneshot::Receiver<proto::SignInCode>,
    ) -> Option<runtime::CredentialSigningIn<'_>> {
        let requested = String::from(kind);
        Some(Box::pin(async move {
            let install::Home::Private(home) = &self.0.home else {
                return Err(String::from("no isolated home"));
            };
            std::fs::write(home.join("credential-sign-in-marker"), "temporary")
                .map_err(|err| err.to_string())?;
            show.send(String::from(URL))
                .map_err(|_| String::from("no link subscriber"))?;
            if code.await.map_err(|err| err.to_string())?.0 != CODE {
                return Err(String::from("incorrect test code"));
            }
            Ok(credential(
                &requested,
                &[("account_label", &label), ("token", TOKEN)],
            ))
        }))
    }
}

fn start(own: &InstanceId) -> CommandBody {
    CommandBody::StartCredentialSignIn {
        instance: own.clone(),
        kind: String::from("setup_token"),
        label: String::from(LABEL),
    }
}

fn submit(own: &InstanceId) -> CommandBody {
    CommandBody::SubmitSignInCode {
        instance: own.clone(),
        code: proto::SignInCode(String::from(CODE)),
    }
}

async fn begin(client: &mut RawClient, own: &InstanceId) -> Fallible {
    client.accepted(start(own)).await?;
    assert_eq!(
        sign_in_until(client, own, |step| matches!(
            step,
            SignInStep::Open { .. }
        ))
        .await?,
        SignInStep::Open {
            url: String::from(URL),
            code: None,
            paste: true,
        }
    );
    Ok(())
}

async fn cancelled(client: &mut RawClient, own: &InstanceId) -> Fallible {
    loop {
        if matches!(
            &client.next().await?.value,
            Feed::SignIn(SignIn { instance, step: SignInStep::Cancelled })
                if instance == own
        ) {
            return Ok(());
        }
    }
}

async fn finish(
    client: &mut RawClient,
    own: &InstanceId,
    account: &InstanceId,
) -> Fallible<Instances> {
    client.accepted(submit(own)).await?;
    let done = sign_in_until(client, own, |step| {
        matches!(step, SignInStep::SignedIn | SignInStep::Failed { .. })
    })
    .await?;
    match done {
        SignInStep::SignedIn => {
            crate::wait_for_offer(client, account).await?;
            Ok(client.instances.clone())
        }
        step => Err(format!("credential sign-in ended at {step:?}").into()),
    }
}

fn assert_private(
    data: &Path,
    log_path: &Path,
    own: &InstanceId,
    account: &InstanceId,
) -> Fallible {
    let roster = disk::roster(data)?;
    assert_eq!(
        roster
            .of(account)
            .and_then(|instance| instance.credential.as_ref())
            .map(|credential| credential.value("token")),
        Some(TOKEN),
    );
    assert!(
        roster
            .of(own)
            .is_some_and(|instance| instance.credential.is_none())
    );
    assert_eq!(disk::receipt_count(data)?, 0);
    let logged = captured(log_path)?;
    for live_only in [URL, CODE, TOKEN] {
        assert!(!logged.contains(live_only));
        assert_eq!(holding(data, live_only)?, Vec::<std::path::PathBuf>::new());
    }
    for entry in std::fs::read_dir(data)? {
        assert!(
            !entry?
                .file_name()
                .to_string_lossy()
                .starts_with("credential-sign-in-")
        );
    }
    Ok(())
}

#[tokio::test]
async fn paired_client_creates_a_credential_that_the_host_keeps() -> Fallible {
    let data = data_dir("paired-credential-sign-in")?;
    let (log_path, logging) = capture_log("paired-credential-sign-in")?;
    let mut plugin = claude_cli::plugin(Settings {
        path: Some(fake_claude()),
        model: None,
    });
    plugin.instance = |launch| Arc::new(Setup(launch));
    let own = instance(Provider::Claude);
    let account = InstanceId::named(Provider::Claude, LABEL);
    let fx = serve(endpoint().await?, std::iter::once(plugin).collect(), &data)
        .await?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;

    begin(&mut phone, &own).await?;
    phone
        .accepted(CommandBody::CancelSignIn {
            instance: own.clone(),
        })
        .await?;
    cancelled(&mut phone, &own).await?;
    assert_eq!(
        phone.send(submit(&own)).await?.value,
        CommandResult::Rejected(Rejection::NoCodeAsked)
    );

    begin(&mut phone, &own).await?;
    let instances = finish(&mut phone, &own, &account).await?;
    assert_eq!(
        instances.availability(&account),
        Some(&Availability::Available {
            auth: Auth::Stored {
                kind: String::from("setup_token"),
                url: None
            },
            models: probed(),
        }),
    );
    assert_eq!(
        raw_history(&fx.target, &mut feed).await?,
        Vec::<proto::Event>::new()
    );
    phone.close().await;
    fx.shutdown().await?;
    drop(logging);
    assert_private(&data, &log_path, &own, &account)?;
    feed.write()
}
