mod claude {
const EXIT_GRACE: Duration = Duration::from_secs(5);
impl Running {
    async fn stop(self) -> Exit {
        let Running {
            mut child,
            stdin,
            stdout: _,
        } = self;
        drop(stdin);
        match tokio::time::timeout(EXIT_GRACE, child.wait()).await {
            Ok(Ok(status)) => Exit::Status(status),
            Ok(Err(_)) | Err(_) => {
                let _ = child.start_kill();
                match child.wait().await {
                    Ok(status) => Exit::Status(status),
                    Err(e) => Exit::Unknown(e.to_string()),
                }
            }
        }
    }
}
}
mod codex {
const EXIT_GRACE: Duration = Duration::from_secs(5);
impl Running {
    async fn stop(self) -> Exit {
        let Running {
            mut child,
            stdin,
            stdout: _,
        } = self;
        drop(stdin);
        match tokio::time::timeout(EXIT_GRACE, child.wait()).await {
            Ok(Ok(status)) => Exit::Status(status),
            Ok(Err(_)) | Err(_) => {
                let _ = child.start_kill();
                match child.wait().await {
                    Ok(status) => Exit::Status(status),
                    Err(e) => Exit::Unknown(e.to_string()),
                }
            }
        }
    }
}
}
