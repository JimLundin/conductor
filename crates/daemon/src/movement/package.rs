use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, ensure};
use host::movement::{Arrival, Incoming, Refs, SessionFiles};
use iroh::endpoint::{RecvStream, SendStream};
use proto::movement::MoveId;
use proto::{
    Epoch, Event, HostPath, HostSeq, InstanceId, SessionId, Workspace,
};
use runtime::Transcript;
use serde::{Deserialize, Serialize};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::workspace::{movement, text};

use super::{Mover, ProviderFiles, data_path};

const MAX_BUNDLE: u64 = 1024 * 1024 * 1024;
const MAX_HISTORY: u64 = 64 * 1024 * 1024;
const CHUNK: usize = 64 * 1024;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Header {
    pub session: SessionId,
    pub id: MoveId,
    source: InstanceId,
    version: String,
    repository: Repository,
    directory: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Repository {
    roots: Vec<String>,
    remote: Option<String>,
    format: host::movement::ObjectFormat,
}

#[derive(Serialize, Deserialize)]
struct History {
    refs: Refs,
    log: SourceLog,
}

#[derive(Serialize, Deserialize)]
struct SourceLog {
    epoch: Epoch,
    last: HostSeq,
    events: Vec<Event>,
    conversation: Option<String>,
    transcript: Option<Transcript>,
}

pub(super) struct Scratch {
    root: PathBuf,
}

impl Scratch {
    pub(super) fn new(data: &Path) -> Result<Self> {
        let base = data.join("move-transfers");
        std::fs::create_dir_all(&base)?;
        let root = base.join(ulid::Ulid::new().to_string());
        std::fs::create_dir(&root)?;
        Ok(Self { root })
    }

    fn bundle(&self) -> Result<PathBuf> {
        Ok(std::fs::canonicalize(&self.root)?.join("workspace.bundle"))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub(super) struct Prepared {
    pub header: Header,
    scratch: Scratch,
    log: SourceLog,
    workspace: Workspace,
    checkpoint: String,
}

impl Prepared {
    pub(super) async fn of(
        mover: &Mover,
        session: SessionId,
        id: MoveId,
        events: Vec<Event>,
        scratch: Scratch,
    ) -> Result<Self> {
        let files = SessionFiles::of(&events).map_err(anyhow::Error::msg)?;
        let last = events.last().context("the source session is missing")?.seq;
        let epoch = mover.log.epoch().await?;
        let conversation = mover.log.saved_conversation(session).await?;
        let (_, provider) =
            mover.provider(files.instance.clone(), false).await?;
        let root = Path::new(&files.workspace.repository.0);
        let repository = Repository::of(root, &files.checkpoint).await?;
        let directory = portable_directory(
            Path::new(&files.workspace.original.0).strip_prefix(root)?,
        )?;
        let native =
            host::movement::conversations(&events, conversation.as_deref());
        let version = provider.version.clone();
        let transcript = export_transcript(provider, session, native).await?;
        Ok(Self {
            header: Header {
                session,
                id,
                source: files.instance,
                version,
                repository,
                directory,
            },
            scratch,
            workspace: files.workspace,
            checkpoint: files.checkpoint,
            log: SourceLog {
                epoch,
                last,
                events,
                conversation,
                transcript,
            },
        })
    }

    pub(super) async fn make_transfer(
        self,
        known: Option<&str>,
    ) -> Result<Transfer> {
        let bundle = self.scratch.bundle()?;
        let refs = movement::export(
            &self.workspace,
            self.header.session,
            &bundle,
            &self.checkpoint,
            known,
        )
        .await?;
        let length = tokio::fs::metadata(&bundle).await?.len();
        ensure!(
            length <= MAX_BUNDLE,
            "the session's Git bundle exceeds the transfer limit"
        );
        let bytes = postcard::to_stdvec(&History {
            refs,
            log: self.log,
        })?;
        let history_len = u64::try_from(bytes.len())?;
        ensure!(
            history_len <= MAX_HISTORY,
            "the session's history exceeds the transfer limit"
        );
        Ok(Transfer {
            _scratch: self.scratch,
            bundle,
            length,
            bytes,
            history_len,
        })
    }
}

fn portable_directory(path: &Path) -> Result<Vec<String>> {
    path.iter()
        .map(|component| {
            let name = component
                .to_str()
                .context("the selected directory must be UTF-8")?;
            ensure!(
                host::movement::portable_name(name),
                "the selected directory name is not portable"
            );
            Ok(name.to_owned())
        })
        .collect()
}

pub(super) struct Transfer {
    _scratch: Scratch,
    bundle: PathBuf,
    length: u64,
    bytes: Vec<u8>,
    history_len: u64,
}

impl Transfer {
    pub(super) async fn send_bundle(
        &self,
        send: &mut SendStream,
    ) -> Result<()> {
        let mut file = File::open(&self.bundle).await?;
        send.write_all(&self.length.to_be_bytes()).await?;
        let mut chunk = vec![0u8; CHUNK];
        loop {
            let read = file.read(&mut chunk).await?;
            if read == 0 {
                return Ok(());
            }
            send.write_all(chunk.get(..read).context("invalid bundle chunk")?)
                .await?;
        }
    }

    pub(super) async fn send_history(
        self,
        send: &mut SendStream,
    ) -> Result<()> {
        send.write_all(&self.history_len.to_be_bytes()).await?;
        send.write_all(&self.bytes).await?;
        Ok(())
    }
}

async fn export_transcript(
    provider: ProviderFiles,
    session: SessionId,
    ids: Vec<String>,
) -> Result<Option<Transcript>> {
    if ids.is_empty() {
        return Ok(None);
    }
    tokio::task::spawn_blocking(move || {
        (provider.io.export)(&provider.home, session, &ids)
            .map(Some)
            .map_err(anyhow::Error::msg)
    })
    .await?
}

pub(super) struct Target {
    source: InstanceId,
    instance: InstanceId,
    provider: ProviderFiles,
    checkout: HostPath,
}

impl Target {
    pub(super) async fn ready(
        mover: &Mover,
        header: Header,
        prior: &[Event],
    ) -> Result<Self> {
        let (instance, provider) =
            mover.provider(header.source.clone(), true).await?;
        ensure!(
            header.version == provider.version,
            "CLI pins differ; update the target first"
        );
        let root = checkout(mover, &header, prior).await?;
        let checkout = selected(
            &root,
            &header.directory,
            header.repository.remote.is_none(),
        )?;
        Ok(Self {
            source: header.source,
            instance,
            provider,
            checkout,
        })
    }

    pub(super) async fn known(&self) -> Option<String> {
        text(
            Path::new(&self.checkout.0),
            &["rev-parse", "--verify", "HEAD^{commit}"],
        )
        .await
        .ok()
    }

    pub(super) fn verify(
        &self,
        incoming: Incoming,
        received: &Received,
        prior: &[Event],
    ) -> Result<()> {
        let history = &received.history;
        host::movement::validate_history(
            incoming.session,
            &self.source,
            &history.refs,
            &history.log.events,
        )
        .map_err(anyhow::Error::msg)?;
        if !prior.is_empty() {
            let retained =
                SessionFiles::of(prior).map_err(anyhow::Error::msg)?;
            host::movement::is_successor(
                incoming.session,
                prior,
                &history.log.events,
                &self.instance,
                &retained.workspace,
            )?;
        }
        Ok(())
    }

    pub(super) async fn install(
        self,
        mover: &Mover,
        received: Received,
        incoming: Incoming,
        previous: Option<&str>,
    ) -> Result<Arrival> {
        let History {
            refs,
            log:
                SourceLog {
                    epoch,
                    last,
                    events,
                    conversation,
                    transcript,
                },
        } = received.history;
        let workspace = movement::import(
            &mover.data,
            &self.checkout,
            incoming,
            &received.scratch.bundle()?,
            &refs,
            previous,
        )
        .await?;
        let ids =
            host::movement::conversations(&events, conversation.as_deref());
        import_transcript(self.provider, incoming.session, ids, transcript)
            .await?;
        Ok(Arrival {
            incoming,
            source_epoch: epoch,
            source_last_seq: last,
            destination: self.instance,
            workspace,
            history: events,
            conversation,
        })
    }
}

pub(super) struct Received {
    scratch: Scratch,
    history: History,
}

impl Received {
    pub(super) async fn receive(
        scratch: Scratch,
        recv: &mut RecvStream,
    ) -> Result<Self> {
        let bundle = scratch.bundle()?;
        receive_bundle(recv, &bundle).await?;
        let history = receive_history(recv).await?;
        Ok(Self { scratch, history })
    }
}

async fn import_transcript(
    provider: ProviderFiles,
    session: SessionId,
    ids: Vec<String>,
    transcript: Option<Transcript>,
) -> Result<()> {
    match (ids.is_empty(), transcript) {
        (true, None) => Ok(()),
        (false, Some(archive)) => {
            tokio::task::spawn_blocking(move || {
                (provider.io.import)(&provider.home, session, &ids, &archive)
                    .map_err(anyhow::Error::msg)
            })
            .await?
        }
        _ => anyhow::bail!(
            "provider transcript does not match the transferred session history"
        ),
    }
}

async fn receive_bundle(recv: &mut RecvStream, path: &Path) -> Result<()> {
    let mut remaining = bounded_size(recv, MAX_BUNDLE).await?;
    let mut file = File::create_new(path).await?;
    let mut chunk = vec![0u8; CHUNK];
    while remaining > 0 {
        let size = usize::try_from(remaining.min(CHUNK as u64))?;
        let next = chunk.get_mut(..size).context("invalid bundle chunk")?;
        recv.read_exact(next).await?;
        file.write_all(next).await?;
        remaining -= u64::try_from(size)?;
    }
    file.sync_all().await?;
    Ok(())
}

async fn receive_history(recv: &mut RecvStream) -> Result<History> {
    let length = usize::try_from(bounded_size(recv, MAX_HISTORY).await?)?;
    let mut body = vec![0u8; length];
    recv.read_exact(&mut body).await?;
    let (history, tail) = postcard::take_from_bytes(&body)?;
    ensure!(tail.is_empty(), "extraneous data after the session history");
    Ok(history)
}

async fn bounded_size(recv: &mut RecvStream, limit: u64) -> Result<u64> {
    let mut header = [0; 8];
    recv.read_exact(&mut header).await?;
    let size = u64::from_be_bytes(header);
    ensure!((1..=limit).contains(&size), "invalid move transfer size");
    Ok(size)
}

impl Repository {
    async fn of(root: &Path, revision: &str) -> Result<Self> {
        let roots =
            text(root, &["rev-list", "--max-parents=0", revision]).await?;
        let remotes = text(root, &["remote"]).await?;
        let remote = if remotes.lines().any(|name| name == "origin") {
            let remote = text(root, &["remote", "get-url", "origin"]).await?;
            Some(home::checkout::normal(&remote))
        } else {
            None
        };
        let raw_format =
            text(root, &["rev-parse", "--show-object-format"]).await?;
        let format = host::movement::ObjectFormat::read(&raw_format)
            .map_err(anyhow::Error::msg)?;
        Ok(Self {
            roots: roots.lines().map(str::to_owned).collect(),
            remote,
            format,
        })
    }
}

async fn checkout(
    mover: &Mover,
    header: &Header,
    prior: &[Event],
) -> Result<PathBuf> {
    match &header.repository.remote {
        Some(remote) => managed_checkout(mover, header, remote).await,
        None => local_checkout(mover, header, prior).await,
    }
}

async fn local_checkout(
    mover: &Mover,
    header: &Header,
    prior: &[Event],
) -> Result<PathBuf> {
    if let Some(previous) = reusable_local(prior, header).await? {
        return Ok(previous);
    }
    let path = data_path(&mover.data, "move-repositories", header.session);
    std::fs::create_dir_all(&path)?;
    let format = format!("--object-format={}", header.repository.format.git());
    text(&path, &["init", "--quiet", &format]).await?;
    Ok(path)
}

async fn reusable_local(
    prior: &[Event],
    header: &Header,
) -> Result<Option<PathBuf>> {
    if prior.is_empty() {
        return Ok(None);
    }
    let previous = SessionFiles::of(prior).map_err(anyhow::Error::msg)?;
    let path = PathBuf::from(previous.workspace.repository.0);
    let observed = Repository::of(&path, &previous.checkpoint).await?;
    ensure!(
        host::movement::compatible_local(
            &header.repository.roots,
            &observed.roots,
            observed.remote.as_deref()
        ),
        "the retained local checkout no longer matches the returning session"
    );
    Ok(Some(path))
}

async fn managed_checkout(
    mover: &Mover,
    header: &Header,
    remote: &str,
) -> Result<PathBuf> {
    let project = ensure_checkout(mover, remote).await?;
    for checkout in project.checkouts {
        let path = PathBuf::from(checkout.path.0);
        let local = Repository::of(&path, "HEAD").await?;
        if host::movement::compatible_repo(
            &header.repository.roots,
            remote,
            &local.roots,
            local.remote.as_deref(),
        ) {
            return Ok(path);
        }
    }
    anyhow::bail!(
        "no target checkout has the source project's origin and root commit"
    )
}

async fn ensure_checkout(
    mover: &Mover,
    remote: &str,
) -> Result<proto::Project> {
    let known = project(&mover.home, remote)?;
    if known.checkouts.is_empty() {
        let _ = mover
            .home
            .order(host::serve::HomeOrder::Clone(known.id), super::fresh_key())
            .await?;
    }
    project(&mover.home, remote)
}

fn project(
    home: &crate::home::HomeClone,
    remote: &str,
) -> Result<proto::Project> {
    let shown = home.shown().context("the target has no home catalog")?;
    shown
        .projects
        .into_iter()
        .find(|project| {
            project.remote.as_deref().is_some_and(|address| {
                home::checkout::normal(address) == remote
            })
        })
        .context("the target has no matching project")
}

fn selected(
    repo: &Path,
    relative: &[String],
    isolated: bool,
) -> Result<HostPath> {
    let root = std::fs::canonicalize(repo)?;
    let selected =
        relative.iter().try_fold(root.clone(), |mut path, part| {
            let native = Path::new(part);
            ensure!(
                host::movement::portable_name(part)
                    && matches!(
                        native.components().next(),
                        Some(Component::Normal(_))
                    ),
                "the source session directory is not portable"
            );
            path.push(part);
            Ok(path)
        })?;
    if isolated {
        std::fs::create_dir_all(&selected)?;
    }
    let canonical = std::fs::canonicalize(selected)?;
    ensure!(
        canonical.starts_with(&root),
        "the target directory escapes its checkout"
    );
    Ok(HostPath(
        canonical
            .to_str()
            .context("the target directory must be UTF-8")?
            .to_owned(),
    ))
}

pub(super) async fn clean(
    session: SessionId,
    files: SessionFiles,
    provider: ProviderFiles,
) -> Result<()> {
    movement::release(&files.workspace, &files.checkpoint).await?;
    if files.conversations.is_empty() {
        return Ok(());
    }
    tokio::task::spawn_blocking(move || {
        (provider.io.remove)(&provider.home, session, &files.conversations)
            .map_err(anyhow::Error::msg)
    })
    .await?
}
