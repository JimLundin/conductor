#![expect(
    clippy::disallowed_types,
    reason = "Independent retirement and pipe runners synchronize access to the same owned OS process handles"
)]

use std::io;
use std::process::ExitStatus;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout, Command};

type Native = Arc<Mutex<Box<dyn ChildWrapper>>>;

#[cfg(all(test, target_os = "linux"))]
mod tests;

#[derive(Debug, Default)]
struct Ownership {
    closed: bool,
    children: Vec<Weak<Mutex<Box<dyn ChildWrapper>>>>,
}

#[derive(Debug, Clone, Default)]
pub struct Processes(Arc<Mutex<Ownership>>);

impl Processes {
    /// Blocks subsequent spawns even when an ordinary provider call is stuck.
    /// # Errors
    /// The ownership lock was poisoned.
    pub fn retire(&self) -> io::Result<()> {
        lock(&self.0)?.closed = true;
        Ok(())
    }

    /// # Errors
    /// A contained process could not be interrupted.
    pub fn interrupt(&self) -> io::Result<()> {
        self.retire()?;
        for child in self.children()? {
            interrupt(&mut **lock(&child)?)?;
        }
        Ok(())
    }

    /// # Errors
    /// A contained process could not be terminated or reaped.
    pub async fn terminate(&self) -> io::Result<()> {
        self.retire()?;
        let children = self.children()?;
        for child in &children {
            kill(child)?;
        }
        for child in children {
            wait(&child).await?;
        }
        Ok(())
    }

    fn children(&self) -> io::Result<Vec<Native>> {
        Ok(lock(&self.0)?
            .children
            .iter()
            .filter_map(Weak::upgrade)
            .collect())
    }
}

fn kill(child: &Native) -> io::Result<()> {
    let mut process = lock(child)?;
    let killed = process.start_kill();
    if process.try_wait()?.is_none() {
        killed?;
    }
    drop(process);
    Ok(())
}

#[cfg(unix)]
fn interrupt(child: &mut dyn ChildWrapper) -> io::Result<()> {
    let interrupted = child.signal(2);
    if child.try_wait()?.is_some() {
        Ok(())
    } else {
        interrupted
    }
}

#[cfg(windows)]
fn interrupt(child: &mut dyn ChildWrapper) -> io::Result<()> {
    let killed = child.start_kill();
    if child.try_wait()?.is_some() {
        Ok(())
    } else {
        killed
    }
}

#[derive(Debug)]
pub struct Child(Native);

impl Child {
    pub fn id(&self) -> Option<u32> {
        self.0.lock().ok().and_then(|child| child.id())
    }

    pub fn start_kill(&self) -> io::Result<()> {
        lock(&self.0)?.start_kill()
    }

    pub async fn wait(&self) -> io::Result<ExitStatus> {
        wait(&self.0).await
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        if let Ok(mut child) = self.0.lock() {
            let _ = child.start_kill();
        }
    }
}

async fn wait(child: &Native) -> io::Result<ExitStatus> {
    loop {
        let waited = lock(child)?.try_wait()?;
        if let Some(status) = waited {
            return Ok(status);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn lock<T>(mutex: &Mutex<T>) -> io::Result<MutexGuard<'_, T>> {
    mutex
        .lock()
        .map_err(|_| io::Error::other("process ownership lock poisoned"))
}

pub(super) fn spawn(
    command: Command,
    scope: &Processes,
) -> io::Result<(Child, ChildStdin, ChildStdout, ChildStderr)> {
    let mut ownership = lock(&scope.0)?;
    if ownership.closed {
        return Err(io::Error::other("instance is retiring"));
    }
    let mut wrapped = CommandWrap::from(command);
    wrapped.wrap(KillOnDrop);
    #[cfg(unix)]
    wrapped.wrap(process_wrap::tokio::ProcessGroup::leader());
    #[cfg(windows)]
    {
        wrapped.wrap(process_wrap::tokio::CreationFlags(
            windows::Win32::System::Threading::CREATE_NO_WINDOW,
        ));
        wrapped.wrap(process_wrap::tokio::JobObject);
    }
    let mut child = wrapped.spawn()?;
    let stdin = child
        .stdin()
        .take()
        .ok_or_else(|| super::unpiped("stdin"))?;
    let stdout = child
        .stdout()
        .take()
        .ok_or_else(|| super::unpiped("stdout"))?;
    let stderr = child
        .stderr()
        .take()
        .ok_or_else(|| super::unpiped("stderr"))?;
    let owned = Arc::new(Mutex::new(child));
    ownership.children.retain(|child| child.strong_count() > 0);
    ownership.children.push(Arc::downgrade(&owned));
    drop(ownership);
    Ok((Child(owned), stdin, stdout, stderr))
}
