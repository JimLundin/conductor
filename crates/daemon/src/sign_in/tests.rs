use std::time::Duration;

use anyhow::{Context, anyhow};
use host::sign_in::Run;
use proto::{InstanceId, Provider};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, duplex};
use tokio::time::timeout;

use super::{CommandResult, Key, Report, Runs, oneshot};

const WAIT: Duration = Duration::from_secs(10);

fn key(provider: Provider) -> Key {
    Key {
        instance: InstanceId::own(provider),
        run: Run(1),
    }
}

async fn blocked_input(
    runs: &mut Runs,
    key: Key,
) -> anyhow::Result<DuplexStream> {
    let (mut writer, reader) = duplex(1);
    writer.write_all(b"a").await?;
    let (started, entered) = oneshot::channel();
    runs.start_write(key, async move {
        let _ = started.send(());
        writer
            .write_all(b"b")
            .await
            .map_err(|error| error.to_string())?;
        Err(String::from("the blocked input unexpectedly accepted data"))
    });
    timeout(WAIT, entered).await??;
    Ok(reader)
}

#[tokio::test]
async fn a_stalled_input_leaves_another_instance_responsive()
-> anyhow::Result<()> {
    let mut runs = Runs::default();
    let stalled = key(Provider::Claude);
    let other = key(Provider::Codex);
    let stalled_reader = blocked_input(&mut runs, stalled.clone()).await?;
    let (answer, listed) = oneshot::channel();
    runs.listen(other.clone(), listed);
    answer
        .send(CommandResult::Accepted)
        .map_err(|_| anyhow!("the listing task stopped"))?;
    let (listed_key, report) = timeout(WAIT, runs.next())
        .await?
        .context("listing report")?;
    assert_eq!(listed_key, other);
    assert!(matches!(report, Report::Listed(CommandResult::Accepted)));

    let mut other_reader = blocked_input(&mut runs, other.clone()).await?;
    runs.kill(&other);
    let mut received = Vec::new();
    timeout(WAIT, other_reader.read_to_end(&mut received)).await??;
    assert_eq!(received.as_slice(), b"a");
    assert!(runs.writes.contains_key(&stalled));

    drop(stalled_reader);
    let (written_key, report) = timeout(WAIT, runs.next())
        .await?
        .context("write failure report")?;
    assert_eq!(written_key, stalled);
    let Report::Written(result) = report else {
        anyhow::bail!("the failed write did not report a result");
    };
    assert!(result.is_err());
    assert!(runs.written(&written_key, result).is_err());
    assert!(!runs.writes.contains_key(&written_key));
    Ok(())
}
