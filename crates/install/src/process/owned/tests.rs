use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::Processes;

#[tokio::test]
async fn termination_reaches_a_busy_child_and_its_descendant()
-> std::io::Result<()> {
    let scope = Processes::default();
    let mut command = Command::new("sh");
    command.args(["-c", "trap '' INT TERM; sleep 300 & echo $!; wait"]);
    let (_child, _input, output, _error) =
        crate::process::piped_owned(command, &scope)?;
    let mut lines = BufReader::new(output).lines();
    let descendant =
        tokio::time::timeout(Duration::from_secs(2), lines.next_line())
            .await??
            .ok_or_else(|| std::io::Error::other("no descendant"))?;
    scope.retire()?;
    assert!(crate::process::piped_owned(Command::new("true"), &scope).is_err());
    scope.interrupt()?;
    tokio::time::timeout(Duration::from_secs(2), scope.terminate()).await??;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let status =
                std::fs::read_to_string(format!("/proc/{descendant}/stat"));
            if status.is_err()
                || status.is_ok_and(|contents| {
                    contents.split_whitespace().nth(2) == Some("Z")
                })
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    Ok(())
}
