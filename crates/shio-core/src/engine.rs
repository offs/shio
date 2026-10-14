mod command;
mod delete;
mod job;
mod preview;
mod request;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use librqbit::Session;
use parking_lot::RwLock;
use tokio::sync::{OnceCell, broadcast, mpsc, oneshot, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

use crate::chunk::{BandwidthLimiter, HostLimits};
use crate::config::AppConfig;
use crate::db::Database;
use crate::error::{Result, ShioError};
use crate::torrent::TorrentManifest;
use crate::types::{
    ArchivePackage, Download, DownloadId, DownloadKind, DownloadProgress, DownloadStatus,
    PackageExtractState, PackageId, TorrentSource, TorrentState,
};
use crate::worker::blocking;

pub use self::command::EngineCommand;
pub(crate) use self::job::{ActiveSlot, MetadataCommit};
use self::job::{Slots, run_http, run_torrent};
pub(crate) use self::request::validate_http_url;
pub use self::request::{
    HttpArchivePartRequest, HttpArchiveSetRequest, HttpDownloadRequest, TorrentDownloadRequest,
};

const EVENT_CAPACITY: usize = 256;
const COMMAND_CAPACITY: usize = 128;
const WORKER_PROGRESS_CAPACITY: usize = 64;
const PROGRESS_FLUSH_INTERVAL: Duration = Duration::from_secs(1);
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_NAME_SUFFIX: u32 = 100;

#[derive(Debug, Clone)]
pub enum ProgressEvent {
    Update(DownloadProgress),
    Changed {
        downloads: Vec<Download>,
        packages: Vec<ArchivePackage>,
    },
    /// A removed package lists its parts in `downloads`.
    Removed {
        downloads: Vec<DownloadId>,
        packages: Vec<PackageId>,
    },
    /// Always the first event of a stream, and sent again after lag.
    Resync {
        downloads: Vec<Download>,
        packages: Vec<ArchivePackage>,
    },
}

// Every event is sent while holding the `rows` write lock, so a snapshot and a subscription
// taken under the read lock see each change exactly once.
struct Shared {
    rows: RwLock<Rows>,
    config: watch::Sender<AppConfig>,
    limiter: BandwidthLimiter,
    hosts: HostLimits,
    session: OnceCell<Arc<Session>>,
    data_dir: PathBuf,
}

#[derive(Default)]
struct Rows {
    downloads: HashMap<DownloadId, Download>,
    packages: HashMap<PackageId, ArchivePackage>,
}

impl Rows {
    fn resync(&self) -> ProgressEvent {
        ProgressEvent::Resync {
            downloads: self.downloads.values().map(snapshot).collect(),
            packages: self.packages.values().cloned().collect(),
        }
    }

    fn changed(&self, downloads: &[DownloadId], packages: &[PackageId]) -> ProgressEvent {
        ProgressEvent::Changed {
            downloads: downloads
                .iter()
                .filter_map(|id| self.downloads.get(id))
                .map(snapshot)
                .collect(),
            packages: packages
                .iter()
                .filter_map(|id| self.packages.get(id))
                .cloned()
                .collect(),
        }
    }

    fn package_of_row(&self, id: DownloadId) -> Option<&ArchivePackage> {
        self.packages.get(&PackageId(id.0))
    }

    fn package_containing(&self, child: DownloadId) -> Option<&ArchivePackage> {
        self.packages
            .values()
            .find(|package| package.child_ids().any(|id| id == child))
    }
}

impl Shared {
    fn refresh_speed_limits(&self) {
        let (http, torrent) = {
            let rows = self.rows.read();
            let transferring = |torrent: bool| {
                rows.downloads
                    .values()
                    .any(|d| d.status.is_transferring() && d.kind.is_torrent() == torrent)
            };
            (transferring(false), transferring(true))
        };
        let limit = self.config.borrow().speed_limit;
        let (http_limit, torrent_limit) = match limit {
            Some(limit) if http && torrent => (Some(limit - limit / 2), Some(limit / 2)),
            limit => (limit, limit),
        };
        self.limiter.set_limit(http_limit);
        if let Some(session) = self.session.get() {
            session
                .ratelimits
                .set_download_bps(crate::torrent::librqbit_rate_limit(torrent_limit));
        }
    }

    async fn session(&self) -> Result<Arc<Session>> {
        let session = self
            .session
            .get_or_try_init(|| async {
                let config = self.config.borrow().clone();
                let owned: Vec<Download> = self
                    .rows
                    .read()
                    .downloads
                    .values()
                    .filter(|d| d.kind.is_torrent() && !d.status.is_terminal())
                    .map(snapshot)
                    .collect();
                crate::torrent::open_session(
                    &self.data_dir,
                    &config.torrent,
                    config.speed_limit,
                    &owned,
                )
                .await
            })
            .await?
            .clone();
        self.refresh_speed_limits();
        Ok(session)
    }
}

fn snapshot(download: &Download) -> Download {
    let DownloadKind::Torrent(torrent) = &download.kind else {
        return download.clone();
    };
    let source = match &torrent.source {
        TorrentSource::Magnet(magnet) => TorrentSource::Magnet(magnet.clone()),
        TorrentSource::File(_) => TorrentSource::File(Vec::new()),
    };
    Download {
        filename: download.filename.clone(),
        save_path: download.save_path.clone(),
        error_message: download.error_message.clone(),
        kind: DownloadKind::Torrent(TorrentState {
            source,
            metadata_bytes: None,
            files: torrent.files.clone(),
            trackers: torrent.trackers.clone(),
            ..*torrent
        }),
        ..*download
    }
}

pub struct ProgressStream {
    shared: Arc<Shared>,
    events: broadcast::Receiver<ProgressEvent>,
    resync: Option<ProgressEvent>,
}

impl ProgressStream {
    fn new(shared: Arc<Shared>, events: &broadcast::Receiver<ProgressEvent>) -> Self {
        let (events, resync) = {
            let rows = shared.rows.read();
            (events.resubscribe(), Some(rows.resync()))
        };
        Self {
            shared,
            events,
            resync,
        }
    }

    pub fn resubscribe(&self) -> Self {
        Self::new(self.shared.clone(), &self.events)
    }

    pub async fn recv(&mut self) -> Option<ProgressEvent> {
        if let Some(resync) = self.resync.take() {
            return Some(resync);
        }
        match self.events.recv().await {
            Ok(event) => Some(event),
            Err(broadcast::error::RecvError::Closed) => None,
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                tracing::warn!("progress stream lagged by {skipped} events");
                let rows = self.shared.rows.read();
                self.events = self.events.resubscribe();
                Some(rows.resync())
            },
        }
    }
}

impl std::fmt::Debug for ProgressStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgressStream").finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Job {
    Download,
    Extract,
    PackageExtract(PackageId),
}

enum Internal {
    Exit {
        id: DownloadId,
        job: Job,
        result: Result<()>,
    },
    ClaimFilename {
        id: DownloadId,
        filename: String,
        reply: oneshot::Sender<Result<String>>,
    },
    TorrentMetadata {
        id: DownloadId,
        manifest: TorrentManifest,
        metadata_bytes: Vec<u8>,
        reply: oneshot::Sender<Result<()>>,
    },
    PreviewDone(u64),
}

struct Worker {
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

#[derive(Clone)]
struct Context {
    shared: Arc<Shared>,
    db: Arc<Database>,
    events: broadcast::Sender<ProgressEvent>,
    loop_tx: mpsc::UnboundedSender<Internal>,
}

pub struct DownloadEngine {
    ctx: Context,
    http: reqwest::Client,
    commands: mpsc::Receiver<EngineCommand>,
    loop_rx: mpsc::UnboundedReceiver<Internal>,
    slots: Arc<Slots>,
    queue: VecDeque<DownloadId>,
    workers: HashMap<DownloadId, Worker>,
    previews: HashMap<u64, CancellationToken>,
    early_cancels: VecDeque<u64>,
    tasks: JoinSet<()>,
    shutdown: CancellationToken,
}

impl std::fmt::Debug for DownloadEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DownloadEngine").finish_non_exhaustive()
    }
}

impl DownloadEngine {
    pub fn new(
        config: AppConfig,
        data_dir: &Path,
    ) -> Result<(Self, mpsc::Sender<EngineCommand>, ProgressStream)> {
        config.validate()?;
        secure_private_directory(data_dir)?;
        let db = Database::open(&data_dir.join("shio.db"))?;
        Self::with_database(config, data_dir.to_path_buf(), db)
    }

    fn with_database(
        config: AppConfig,
        data_dir: PathBuf,
        db: Database,
    ) -> Result<(Self, mpsc::Sender<EngineCommand>, ProgressStream)> {
        delete::recover(&db)?;
        let http = crate::worker::build_client(&config)?;
        let (rows, queue) = restore(&db)?;
        let (command_tx, commands) = mpsc::channel(COMMAND_CAPACITY);
        let (loop_tx, loop_rx) = mpsc::unbounded_channel();
        let (events, events_rx) = broadcast::channel(EVENT_CAPACITY);
        let shared = Arc::new(Shared {
            rows: RwLock::new(rows),
            limiter: BandwidthLimiter::new(config.speed_limit),
            hosts: HostLimits::default(),
            config: watch::Sender::new(config),
            session: OnceCell::new(),
            data_dir,
        });
        let stream = ProgressStream::new(shared.clone(), &events_rx);
        let engine = Self {
            ctx: Context {
                shared,
                db: Arc::new(db),
                events,
                loop_tx,
            },
            http,
            commands,
            loop_rx,
            slots: Arc::default(),
            queue,
            workers: HashMap::new(),
            previews: HashMap::new(),
            early_cancels: VecDeque::new(),
            tasks: JoinSet::new(),
            shutdown: CancellationToken::new(),
        };
        Ok((engine, command_tx, stream))
    }

    pub async fn run(mut self) {
        let reply = loop {
            self.start_queued();
            while self.tasks.try_join_next().is_some() {}
            tokio::select! {
                command = self.commands.recv() => match command {
                    Some(EngineCommand::Shutdown { reply }) => break Some(reply),
                    Some(command) => self.handle(command).await,
                    None => break None,
                },
                Some(message) = self.loop_rx.recv() => self.handle_internal(message),
                () = self.slots.freed.notified() => {},
            }
        };
        self.stop().await;
        if let Some(reply) = reply {
            let _ = reply.send(());
        }
    }

    async fn handle(&mut self, command: EngineCommand) {
        let (reply, result) = match command {
            EngineCommand::AddHttp { request, reply } => (reply, self.add_http(request)),
            EngineCommand::AddHttpArchiveSet { request, reply } => {
                (reply, self.add_archive_set(request))
            },
            EngineCommand::AddTorrentPrepared { request, reply } => {
                (reply, self.add_torrent(request))
            },
            EngineCommand::Pause { id, reply } => (reply, self.pause(id)),
            EngineCommand::Resume { id, reply } => (reply, self.resume(id)),
            EngineCommand::Cancel { id, reply } => (reply, self.cancel(id)),
            EngineCommand::Remove {
                id,
                delete_files,
                reply,
            } => (reply, self.remove(id, delete_files).await),
            EngineCommand::Retry { id, reply } => (reply, self.retry(id)),
            EngineCommand::RetryExtract {
                id,
                password,
                reply,
            } => (reply, self.retry_extract(id, password)),
            EngineCommand::ForceRecheck { id, reply } => (reply, self.force_recheck(id).await),
            EngineCommand::StopSeeding { id, reply } => (reply, self.stop_seeding(id)),
            EngineCommand::SetPin { id, pinned, reply } => (reply, self.set_pin(id, pinned)),
            EngineCommand::PauseAll { reply } => (reply, self.pause_all()),
            EngineCommand::ResumeAll { reply } => (reply, self.resume_all()),
            EngineCommand::SetConfig { config, reply } => (reply, self.set_config(config)),
            EngineCommand::ResolveMagnetPreview {
                request_id,
                magnet,
                reply,
            } => return self.resolve_magnet_preview(request_id, magnet, reply),
            EngineCommand::ResolveHttpPreview {
                request_id,
                url,
                reply,
            } => return self.resolve_http_preview(request_id, url, reply),
            EngineCommand::CancelPreview { request_id } => return self.cancel_preview(request_id),
            EngineCommand::Shutdown { .. } => unreachable!("run handles shutdown"),
        };
        if reply.send(result).is_err() {
            tracing::debug!("command reply dropped");
        }
    }

    fn handle_internal(&mut self, message: Internal) {
        match message {
            Internal::Exit { id, job, result } => self.finish(id, job, &result),
            Internal::ClaimFilename {
                id,
                filename,
                reply,
            } => {
                let _ = reply.send(self.claim_filename(id, &filename));
            },
            Internal::TorrentMetadata {
                id,
                manifest,
                metadata_bytes,
                reply,
            } => {
                let _ = reply.send(self.commit_torrent_metadata(id, manifest, metadata_bytes));
            },
            Internal::PreviewDone(request_id) => {
                self.previews.remove(&request_id);
            },
        }
    }

    fn snapshots(&self, ids: &[DownloadId]) -> Vec<Download> {
        let rows = self.ctx.shared.rows.read();
        ids.iter()
            .filter_map(|id| rows.downloads.get(id))
            .map(snapshot)
            .collect()
    }

    fn snapshot(&self, id: DownloadId) -> Result<Download> {
        self.ctx
            .shared
            .rows
            .read()
            .downloads
            .get(&id)
            .map(snapshot)
            .ok_or_else(|| ShioError::Other("download not found".into()))
    }

    // Byte counters stay as the progress tracker last set them.
    fn commit(&self, rows: &[Download]) -> Result<()> {
        self.ctx.db.update_downloads(rows)?;
        self.apply(rows);
        Ok(())
    }

    fn apply(&self, rows: &[Download]) {
        let mut state = self.ctx.shared.rows.write();
        for row in rows {
            if let Some(current) = state.downloads.get_mut(&row.id) {
                current.status = row.status;
                current.error_message.clone_from(&row.error_message);
                current.retry_count = row.retry_count;
                current.started_at = row.started_at;
                current.completed_at = row.completed_at;
                if !row.status.is_active() {
                    current.speed = 0;
                }
            }
        }
        let ids: Vec<_> = rows.iter().map(|row| row.id).collect();
        let _ = self.ctx.events.send(state.changed(&ids, &[]));
    }

    fn commit_package(&self, package: ArchivePackage) -> Result<()> {
        self.ctx.db.update_package(&package)?;
        let mut rows = self.ctx.shared.rows.write();
        let id = package.id;
        rows.packages.insert(id, package);
        let _ = self.ctx.events.send(rows.changed(&[], &[id]));
        Ok(())
    }

    fn set_status(
        &self,
        ids: &[DownloadId],
        status: DownloadStatus,
        edit: impl Fn(&mut Download),
    ) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut rows = self.snapshots(ids);
        for row in &mut rows {
            row.status = status;
            row.error_message = None;
            edit(row);
        }
        self.commit(&rows)
    }

    fn command_targets(
        &self,
        id: DownloadId,
        allowed: impl Fn(DownloadStatus) -> bool,
        action: &str,
    ) -> Result<Vec<DownloadId>> {
        let rows = self.ctx.shared.rows.read();
        let candidates: Vec<DownloadId> = match rows.package_of_row(id) {
            Some(package) => package.child_ids().collect(),
            None if rows.downloads.contains_key(&id) => vec![id],
            None => return Err(ShioError::Other("download not found".into())),
        };
        let targets: Vec<DownloadId> = candidates
            .into_iter()
            .filter(|id| rows.downloads.get(id).is_some_and(|d| allowed(d.status)))
            .collect();
        if targets.is_empty() {
            return Err(ShioError::Other(format!("nothing to {action}")));
        }
        Ok(targets)
    }

    fn add_http(&mut self, request: HttpDownloadRequest) -> Result<()> {
        let download = request.into_download(&self.ctx.shared.config.borrow())?;
        self.reject_collisions(&[&download], true)?;
        self.insert(vec![download], None)
    }

    fn add_archive_set(&mut self, request: HttpArchiveSetRequest) -> Result<()> {
        let (package, downloads) = request.into_rows(&self.ctx.shared.config.borrow())?;
        self.reject_collisions(&downloads.iter().collect::<Vec<_>>(), true)?;
        self.insert(downloads, Some(package))
    }

    fn add_torrent(&mut self, request: TorrentDownloadRequest) -> Result<()> {
        let download = request.into_download()?;
        self.reject_active_duplicate(&download)?;
        self.reject_collisions(&[&download], true)?;
        self.insert(vec![download], None)
    }

    // Restore refuses a database with two unfinished rows for one torrent.
    fn reject_active_duplicate(&self, download: &Download) -> Result<()> {
        let info_hash = download.torrent().map(|torrent| torrent.info_hash);
        let duplicate = self.ctx.shared.rows.read().downloads.values().any(|d| {
            d.id != download.id
                && !d.status.is_terminal()
                && d.torrent().map(|torrent| torrent.info_hash) == info_hash
        });
        if duplicate {
            return Err(ShioError::Other(
                "torrent is already in the download list".into(),
            ));
        }
        Ok(())
    }

    fn insert(
        &mut self,
        mut downloads: Vec<Download>,
        package: Option<ArchivePackage>,
    ) -> Result<()> {
        for download in &mut downloads {
            download.status = DownloadStatus::Queued;
        }
        match &package {
            Some(package) => self.ctx.db.insert_package(package, &downloads)?,
            None => self.ctx.db.insert_download(&downloads[0])?,
        }
        let ids: Vec<_> = downloads.iter().map(|d| d.id).collect();
        let package_ids: Vec<_> = package.iter().map(|package| package.id).collect();
        let mut rows = self.ctx.shared.rows.write();
        rows.downloads
            .extend(downloads.into_iter().map(|d| (d.id, d)));
        rows.packages
            .extend(package.map(|package| (package.id, package)));
        let _ = self.ctx.events.send(rows.changed(&ids, &package_ids));
        drop(rows);
        self.queue.extend(ids);
        Ok(())
    }

    // Rejects payloads (and, for new rows, staging) that exist on disk or overlap a path
    // another row owns; paths compare case-insensitively.
    fn reject_collisions(&self, candidates: &[&Download], new_rows: bool) -> Result<()> {
        let mut claimed: Vec<(Vec<String>, PathBuf)> = Vec::new();
        for candidate in candidates {
            let staging = new_rows.then(|| staging_root(candidate));
            for path in payload_paths(candidate).into_iter().chain(staging) {
                let path = std::path::absolute(&path).unwrap_or(path);
                match std::fs::symlink_metadata(&path) {
                    Ok(_) => return Err(ShioError::DestinationExists(path)),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                    Err(error) => return Err(error.into()),
                }
                let key = path_key(&path)?;
                if claimed.iter().any(|(other, _)| overlaps(&key, other)) {
                    return Err(ShioError::DestinationExists(path));
                }
                claimed.push((key, path));
            }
        }
        let rows = self.ctx.shared.rows.read();
        let others = rows
            .downloads
            .values()
            .filter(|d| candidates.iter().all(|candidate| candidate.id != d.id));
        for other in others {
            for path in payload_paths(other)
                .into_iter()
                .chain([staging_root(other)])
            {
                let key = path_key(&std::path::absolute(&path).unwrap_or(path))?;
                if let Some((_, path)) = claimed.iter().find(|(new, _)| overlaps(new, &key)) {
                    return Err(ShioError::DestinationExists(path.clone()));
                }
            }
        }
        Ok(())
    }

    fn pause(&mut self, id: DownloadId) -> Result<()> {
        let ids = self.command_targets(id, is_pausable, "pause")?;
        self.set_status(&ids, DownloadStatus::Paused, |_| {})?;
        self.interrupt(&ids);
        Ok(())
    }

    fn resume(&mut self, id: DownloadId) -> Result<()> {
        let ids = self.command_targets(
            id,
            |status| matches!(status, DownloadStatus::Paused | DownloadStatus::Error),
            "resume",
        )?;
        self.set_status(&ids, DownloadStatus::Queued, |_| {})?;
        self.queue.extend(ids);
        Ok(())
    }

    fn retry(&mut self, id: DownloadId) -> Result<()> {
        let ids = self.command_targets(id, |status| status == DownloadStatus::Error, "retry")?;
        self.set_status(&ids, DownloadStatus::Queued, |row| {
            row.retry_count = row.retry_count.saturating_add(1);
        })?;
        self.queue.extend(ids);
        Ok(())
    }

    fn cancel(&mut self, id: DownloadId) -> Result<()> {
        let extracting = {
            let rows = self.ctx.shared.rows.read();
            rows.package_of_row(id)
                .is_some_and(|package| package.extract_state == PackageExtractState::Extracting)
                || rows
                    .downloads
                    .get(&id)
                    .is_some_and(|d| d.status == DownloadStatus::Extracting)
        };
        if extracting {
            let worker = self
                .workers
                .get(&id)
                .ok_or_else(|| ShioError::Other("extraction is not active".into()))?;
            worker.cancel.cancel();
            return Ok(());
        }
        let ids = self.command_targets(
            id,
            |status| {
                matches!(
                    status,
                    DownloadStatus::Queued
                        | DownloadStatus::Starting
                        | DownloadStatus::Downloading
                        | DownloadStatus::FetchingMetadata
                        | DownloadStatus::Paused
                        | DownloadStatus::Error
                )
            },
            "cancel",
        )?;
        self.set_status(&ids, DownloadStatus::Cancelled, |_| {})?;
        for row in self.snapshots(&ids) {
            if row.kind.is_torrent() && !self.workers.contains_key(&row.id) {
                self.forget_torrent(row, false);
            }
        }
        self.interrupt(&ids);
        Ok(())
    }

    fn stop_seeding(&mut self, id: DownloadId) -> Result<()> {
        let row = self.snapshot(id)?;
        if row.status != DownloadStatus::Seeding {
            return Err(ShioError::Other("download is not seeding".into()));
        }
        self.set_status(&[id], DownloadStatus::Completed, |row| {
            row.completed_at.get_or_insert_with(chrono::Utc::now);
        })?;
        self.interrupt(&[id]);
        Ok(())
    }

    fn ids_where(&self, predicate: impl Fn(DownloadStatus) -> bool) -> Vec<DownloadId> {
        let rows = self.ctx.shared.rows.read();
        rows.downloads
            .values()
            .filter(|d| predicate(d.status))
            .map(|d| d.id)
            .collect()
    }

    fn pause_all(&mut self) -> Result<()> {
        let ids = self.ids_where(is_pausable);
        self.set_status(&ids, DownloadStatus::Paused, |_| {})?;
        self.interrupt(&ids);
        Ok(())
    }

    fn resume_all(&mut self) -> Result<()> {
        let ids = self.ids_where(|status| status == DownloadStatus::Paused);
        self.set_status(&ids, DownloadStatus::Queued, |_| {})?;
        self.queue.extend(ids);
        Ok(())
    }

    // Writes only the flag: a snapshot's status may be stale against worker progress.
    fn set_pin(&self, id: DownloadId, pinned: bool) -> Result<()> {
        let package = self.ctx.shared.rows.read().package_of_row(id).cloned();
        if let Some(mut package) = package {
            package.pinned = pinned;
            return self.commit_package(package);
        }
        self.snapshot(id)?;
        self.ctx.db.update_pinned(id, pinned)?;
        let mut rows = self.ctx.shared.rows.write();
        if let Some(row) = rows.downloads.get_mut(&id) {
            row.pinned = pinned;
        }
        let _ = self.ctx.events.send(rows.changed(&[id], &[]));
        Ok(())
    }

    fn set_config(&mut self, config: AppConfig) -> Result<()> {
        config.validate()?;
        if self.ctx.shared.config.borrow().proxy != config.proxy {
            self.http = crate::worker::build_client(&config)?;
        }
        self.ctx.shared.config.send_replace(config);
        self.ctx.shared.refresh_speed_limits();
        Ok(())
    }

    fn interrupt(&mut self, ids: &[DownloadId]) {
        self.queue.retain(|id| !ids.contains(id));
        for id in ids {
            if let Some(worker) = self.workers.get(id) {
                worker.cancel.cancel();
            }
        }
    }

    fn retry_extract(&mut self, id: DownloadId, password: Option<String>) -> Result<()> {
        let package = self.ctx.shared.rows.read().package_of_row(id).map(|p| p.id);
        if let Some(package) = package {
            return self.start_package_extract(package, password);
        }
        let download = self
            .ctx
            .shared
            .rows
            .read()
            .downloads
            .get(&id)
            .cloned()
            .ok_or_else(|| ShioError::Other("download not found".into()))?;
        if !matches!(
            download.status,
            DownloadStatus::ExtractError | DownloadStatus::PasswordRequired
        ) {
            return Err(ShioError::Other("download is not extractable".into()));
        }
        self.set_status(&[id], DownloadStatus::Extracting, |_| {})?;
        let config = self.ctx.shared.config.borrow().clone();
        let cancel = self.shutdown.child_token();
        let token = cancel.clone();
        self.spawn_job(id, Job::Extract, cancel, async move {
            crate::worker::run_extract(std::slice::from_ref(&download), &config, password, token)
                .await
        });
        Ok(())
    }

    fn start_package_extract(&mut self, id: PackageId, password: Option<String>) -> Result<()> {
        let (mut package, parts) = {
            let rows = self.ctx.shared.rows.read();
            let package = rows
                .packages
                .get(&id)
                .cloned()
                .ok_or_else(|| ShioError::Other("package not found".into()))?;
            let parts: Vec<Download> = package
                .child_ids()
                .filter_map(|child| rows.downloads.get(&child))
                .filter(|d| d.status == DownloadStatus::Completed)
                .cloned()
                .collect();
            (package, parts)
        };
        let ready = package.auto_extract
            && parts.len() == package.items.len()
            && !matches!(
                package.extract_state,
                PackageExtractState::Extracting | PackageExtractState::Completed
            );
        if !ready || self.workers.contains_key(&id.row_id()) {
            return Err(ShioError::Other(
                "package is not ready for extraction".into(),
            ));
        }
        package.extract_state = PackageExtractState::Extracting;
        package.error_message = None;
        self.commit_package(package)?;
        let config = self.ctx.shared.config.borrow().clone();
        let cancel = self.shutdown.child_token();
        let token = cancel.clone();
        self.spawn_job(id.row_id(), Job::PackageExtract(id), cancel, async move {
            crate::worker::run_extract(&parts, &config, password, token).await
        });
        Ok(())
    }

    async fn remove(&mut self, id: DownloadId, delete_files: bool) -> Result<()> {
        let (package, ids) = {
            let rows = self.ctx.shared.rows.read();
            if let Some(package) = rows.package_of_row(id) {
                (Some(package.clone()), package.child_ids().collect())
            } else if rows.package_containing(id).is_some() {
                return Err(ShioError::Other(
                    "archive parts are removed with their archive set".into(),
                ));
            } else if rows.downloads.contains_key(&id) {
                (None, vec![id])
            } else {
                return Err(ShioError::Other("download not found".into()));
            }
        };
        for child in &ids {
            self.stop_worker(*child).await?;
        }
        // A part that completed while stopping may have started the package extraction.
        if let Some(package) = &package {
            self.stop_worker(package.id.row_id()).await?;
        }
        let downloads = self.snapshots(&ids);
        let mut targets = Vec::new();
        if delete_files {
            for download in &downloads {
                targets.push(delete::targets(download).await?);
            }
        }
        for download in &downloads {
            self.stop_torrent_session(download).await;
        }
        let db = self.ctx.db.clone();
        let quarantines = blocking(move || delete::quarantine(&db, &targets)).await?;
        let deleted = match &package {
            Some(package) => self.ctx.db.delete_package(package),
            None => self.ctx.db.delete_download(id),
        };
        let db = self.ctx.db.clone();
        if let Err(error) = deleted {
            blocking(move || {
                delete::rollback(&db, quarantines);
                Ok(())
            })
            .await?;
            return Err(error);
        }
        {
            let mut rows = self.ctx.shared.rows.write();
            for id in &ids {
                rows.downloads.remove(id);
            }
            let package_ids: Vec<_> = package.iter().map(|package| package.id).collect();
            for package in &package_ids {
                rows.packages.remove(package);
            }
            let _ = self.ctx.events.send(ProgressEvent::Removed {
                downloads: ids.clone(),
                packages: package_ids,
            });
        }
        self.queue.retain(|queued| !ids.contains(queued));
        // Staging goes first so the directories it lived in can be removed when empty.
        for download in downloads.iter().filter(|d| d.kind.is_torrent()) {
            if let Err(error) = crate::torrent::cleanup_staging(download).await {
                tracing::warn!(download = %download.id, "removed row left torrent staging behind: {error}");
            }
        }
        blocking(move || {
            for download in downloads.iter().filter(|d| d.kind.is_http()) {
                if let Err(error) = delete::remove_http_staging(download) {
                    tracing::warn!(download = %download.id, "removed row left HTTP staging behind: {error}");
                }
            }
            delete::commit(&db, quarantines);
            Ok(())
        })
        .await
    }

    async fn force_recheck(&mut self, id: DownloadId) -> Result<()> {
        let row = self.snapshot(id)?;
        if !row.kind.is_torrent()
            || !matches!(
                row.status,
                DownloadStatus::Paused
                    | DownloadStatus::Completed
                    | DownloadStatus::Seeding
                    | DownloadStatus::Error
            )
        {
            return Err(ShioError::Other("torrent cannot be rechecked now".into()));
        }
        if row.status == DownloadStatus::Completed {
            self.reject_active_duplicate(&row)?;
        }
        // Queued first, so the stopping worker's exit keeps the row queued.
        self.set_status(&[id], DownloadStatus::Queued, |_| {})?;
        if let Err(error) = self.stop_worker(id).await {
            self.queue.push_back(id);
            return Err(error);
        }
        self.stop_torrent_session(&row).await;
        if row.status == DownloadStatus::Completed
            && let Err(error) = crate::torrent::prepare_completed_recheck(&row).await
        {
            self.set_status(&[id], DownloadStatus::Error, |row| {
                row.error_message = Some(error.short_label());
            })?;
            return Err(error);
        }
        self.queue.push_back(id);
        Ok(())
    }

    async fn stop_worker(&mut self, id: DownloadId) -> Result<()> {
        let Some(worker) = self.workers.get(&id) else {
            return Ok(());
        };
        worker.cancel.cancel();
        let deadline = tokio::time::Instant::now() + STOP_TIMEOUT;
        while self.workers.contains_key(&id) {
            match tokio::time::timeout_at(deadline, self.loop_rx.recv()).await {
                Ok(Some(message)) => self.handle_internal(message),
                Ok(None) | Err(_) => {
                    return Err(ShioError::Other(
                        "timed out waiting for the download to stop".into(),
                    ));
                },
            }
        }
        Ok(())
    }

    async fn stop_torrent_session(&self, download: &Download) {
        if let (Some(session), Some(torrent)) = (self.ctx.shared.session.get(), download.torrent())
            && let Err(error) =
                crate::torrent::stop_torrent_in_session(session, torrent.info_hash).await
        {
            tracing::warn!(download = %download.id, "failed to remove torrent from session: {error}");
        }
    }

    // Without an open session there is nothing to do: the next open keeps only owned rows.
    fn forget_torrent(&mut self, download: Download, cleanup_staging: bool) {
        let (Some(session), Some(info_hash)) = (
            self.ctx.shared.session.get().cloned(),
            download.torrent().map(|torrent| torrent.info_hash),
        ) else {
            return;
        };
        self.tasks.spawn(async move {
            if let Err(error) = crate::torrent::stop_torrent_in_session(&session, info_hash).await {
                tracing::warn!(download = %download.id, "failed to remove torrent from session: {error}");
            } else if cleanup_staging
                && let Err(error) = crate::torrent::cleanup_staging(&download).await
            {
                tracing::warn!(download = %download.id, "failed to remove torrent staging: {error}");
            }
        });
    }

    fn start_queued(&mut self) {
        let max = usize::from(self.ctx.shared.config.borrow().max_concurrent);
        while self.slots.active.load(Ordering::Relaxed) < max {
            let next = {
                let rows = self.ctx.shared.rows.read();
                self.queue.retain(|id| {
                    rows.downloads
                        .get(id)
                        .is_some_and(|d| d.status == DownloadStatus::Queued)
                });
                // A row resumed while its previous worker still drains waits for that exit.
                self.queue
                    .iter()
                    .position(|id| !self.workers.contains_key(id))
            };
            let Some(id) = next.and_then(|index| self.queue.remove(index)) else {
                break;
            };
            self.start(id);
        }
    }

    fn start(&mut self, id: DownloadId) {
        let Some(mut download) = self.ctx.shared.rows.read().downloads.get(&id).cloned() else {
            return;
        };
        download.status = DownloadStatus::Starting;
        download.started_at = Some(chrono::Utc::now());
        download.error_message = None;
        if let Err(error) = self.commit(std::slice::from_ref(&download)) {
            self.fail(id, &error);
            return;
        }
        let cancel = self.shutdown.child_token();
        let slot = ActiveSlot::new(&self.slots);
        let ctx = self.ctx.clone();
        let token = cancel.clone();
        if download.kind.is_http() {
            let may_rename = self.ctx.shared.rows.read().package_containing(id).is_none();
            let http = self.http.clone();
            self.spawn_job(id, Job::Download, cancel, async move {
                let _slot = slot;
                run_http(ctx, http, download, may_rename, token).await
            });
        } else {
            self.spawn_job(
                id,
                Job::Download,
                cancel,
                run_torrent(ctx, download, token, slot),
            );
        }
        self.ctx.shared.refresh_speed_limits();
    }

    fn spawn_job(
        &mut self,
        id: DownloadId,
        job: Job,
        cancel: CancellationToken,
        work: impl Future<Output = Result<()>> + Send + 'static,
    ) {
        let loop_tx = self.ctx.loop_tx.clone();
        let task = tokio::spawn(async move {
            let result = work.await;
            let _ = loop_tx.send(Internal::Exit { id, job, result });
        });
        self.workers.insert(id, Worker { cancel, task });
    }

    fn fail(&self, id: DownloadId, error: &ShioError) {
        let Ok(mut row) = self.snapshot(id) else {
            return;
        };
        row.status = DownloadStatus::Error;
        row.error_message = Some(error.short_label());
        if let Err(db_error) = self.ctx.db.update_downloads(std::slice::from_ref(&row)) {
            row.error_message = Some(format!("database write failed: {}", db_error.short_label()));
        }
        self.apply(&[row]);
    }

    fn finish(&mut self, id: DownloadId, job: Job, result: &Result<()>) {
        let Some(worker) = self.workers.remove(&id) else {
            return;
        };
        let cancelled = worker.cancel.is_cancelled();
        if let Job::PackageExtract(package) = job {
            self.finish_package_extract(package, result);
            return;
        }
        let Ok(mut row) = self.snapshot(id) else {
            return;
        };
        if let Some((status, error)) = exit_status(
            job,
            row.status,
            result,
            cancelled,
            self.shutdown.is_cancelled(),
        ) {
            row.status = status;
            row.error_message = error;
        }
        if row.status.is_downloaded() {
            row.completed_at.get_or_insert_with(chrono::Utc::now);
        }
        let settled = row.kind.is_http()
            && row.status.is_downloaded()
            && crate::worker::settle_publication(&mut row);
        let committed = match self.ctx.db.update_downloads(std::slice::from_ref(&row)) {
            Ok(()) => true,
            Err(error) => {
                row.status = DownloadStatus::Error;
                row.error_message = Some(format!("database write failed: {}", error.short_label()));
                false
            },
        };
        if settled && committed {
            // The worker has exited, so no progress update can overwrite the final size.
            if let Some(current) = self.ctx.shared.rows.write().downloads.get_mut(&id) {
                current.total_size = row.total_size;
                current.downloaded = row.downloaded;
            }
            remove_settled_staging(&row);
        }
        self.apply(std::slice::from_ref(&row));
        self.ctx.shared.refresh_speed_limits();
        if row.status == DownloadStatus::Completed && !self.shutdown.is_cancelled() {
            let package = self
                .ctx
                .shared
                .rows
                .read()
                .package_containing(id)
                .map(|package| package.id);
            if let Some(package) = package
                && let Err(error) = self.start_package_extract(package, None)
            {
                tracing::debug!(%package, "package extraction not started: {error}");
            }
        }
        if cancelled
            && row.kind.is_torrent()
            && matches!(
                row.status,
                DownloadStatus::Cancelled | DownloadStatus::Completed
            )
        {
            let cleanup = row.status == DownloadStatus::Completed;
            self.forget_torrent(row, cleanup);
        }
    }

    fn finish_package_extract(&self, id: PackageId, result: &Result<()>) {
        let Some(mut package) = self.ctx.shared.rows.read().packages.get(&id).cloned() else {
            return;
        };
        (package.extract_state, package.error_message) = match result {
            Ok(()) => (PackageExtractState::Completed, None),
            Err(error @ ShioError::PasswordRequired) => (
                PackageExtractState::PasswordRequired,
                Some(error.short_label()),
            ),
            Err(error) => (PackageExtractState::Error, Some(error.short_label())),
        };
        if package.extract_state == PackageExtractState::Completed {
            package.completed_at = Some(chrono::Utc::now());
        }
        if let Err(error) = self.commit_package(package.clone()) {
            package.extract_state = PackageExtractState::Error;
            package.error_message = Some(format!("database write failed: {}", error.short_label()));
            let mut rows = self.ctx.shared.rows.write();
            rows.packages.insert(id, package);
            let _ = self.ctx.events.send(rows.changed(&[], &[id]));
        }
    }

    // Numbers a server-provided name when it is taken; the worker publishes under the result.
    fn claim_filename(&self, id: DownloadId, filename: &str) -> Result<String> {
        let mut row = self.snapshot(id)?;
        let original = row.filename.clone();
        let available = (0..MAX_NAME_SUFFIX)
            .map(|n| numbered(filename, n))
            .find(|name| {
                row.filename.clone_from(name);
                crate::path::validate_leaf_filename(name).is_ok()
                    && self.reject_collisions(&[&row], false).is_ok()
            });
        let Some(name) = available else {
            return Ok(original);
        };
        self.ctx.db.update_filename(id, &name)?;
        let mut rows = self.ctx.shared.rows.write();
        if let Some(current) = rows.downloads.get_mut(&id) {
            current.filename.clone_from(&name);
        }
        let _ = self.ctx.events.send(rows.changed(&[id], &[]));
        Ok(name)
    }

    fn commit_torrent_metadata(
        &self,
        id: DownloadId,
        manifest: TorrentManifest,
        metadata_bytes: Vec<u8>,
    ) -> Result<()> {
        let mut row = self.snapshot(id)?;
        let Some(torrent) = row.torrent_mut() else {
            return Err(ShioError::Other("download is not a torrent".into()));
        };
        if manifest.info_hash != torrent.info_hash {
            return Err(ShioError::Other(
                "torrent metadata does not match its info hash".into(),
            ));
        }
        let unresolved = torrent.files.is_empty();
        if unresolved {
            torrent.files = manifest.files;
        }
        torrent.is_private = manifest.is_private;
        torrent.trackers = manifest.trackers;
        torrent.metadata_bytes = Some(metadata_bytes);
        crate::torrent::validate_torrent_state(torrent)?;
        let total = request::selected_size(&torrent.files);
        row.total_size = Some(total);
        if unresolved {
            row.filename = crate::filename::sanitize_filename(&manifest.name);
        }
        crate::path::validate_leaf_filename(&row.filename)?;
        self.reject_collisions(&[&row], false)?;
        self.ctx.db.update_torrent_metadata(&row)?;
        let mut rows = self.ctx.shared.rows.write();
        if let Some(current) = rows.downloads.get_mut(&id) {
            current.filename = row.filename;
            current.total_size = row.total_size;
            if let (Some(current), DownloadKind::Torrent(committed)) =
                (current.torrent_mut(), row.kind)
            {
                current.files = committed.files;
                current.trackers = committed.trackers;
                current.is_private = committed.is_private;
                current.metadata_bytes = committed.metadata_bytes;
            }
        }
        let _ = self.ctx.events.send(rows.changed(&[id], &[]));
        Ok(())
    }

    async fn stop(mut self) {
        self.shutdown.cancel();
        let deadline = tokio::time::Instant::now() + STOP_TIMEOUT;
        while !self.workers.is_empty() {
            match tokio::time::timeout_at(deadline, self.loop_rx.recv()).await {
                Ok(Some(message)) => self.handle_internal(message),
                Ok(None) | Err(_) => break,
            }
        }
        for (id, worker) in self.workers.drain() {
            tracing::warn!(download = %id, "worker did not stop in time; aborting");
            worker.task.abort();
        }
        if tokio::time::timeout_at(deadline, async {
            while self.tasks.join_next().await.is_some() {}
        })
        .await
        .is_err()
        {
            self.tasks.shutdown().await;
        }
        if let Err(error) = self.ctx.db.checkpoint() {
            tracing::warn!("database checkpoint failed: {error}");
        }
    }
}

const fn is_pausable(status: DownloadStatus) -> bool {
    matches!(
        status,
        DownloadStatus::Queued
            | DownloadStatus::Starting
            | DownloadStatus::Downloading
            | DownloadStatus::FetchingMetadata
            | DownloadStatus::Seeding
    )
}

// `None` keeps the status a command already committed (pause, cancel, stop seeding, recheck).
fn exit_status(
    job: Job,
    current: DownloadStatus,
    result: &Result<()>,
    cancelled: bool,
    shutdown: bool,
) -> Option<(DownloadStatus, Option<String>)> {
    let Err(error) = result else {
        return Some((DownloadStatus::Completed, None));
    };
    if cancelled {
        return match current {
            status if shutdown && job == Job::Download && status.is_active() => {
                Some((DownloadStatus::Queued, None))
            },
            DownloadStatus::Extracting => Some((
                DownloadStatus::ExtractError,
                Some("extraction cancelled".into()),
            )),
            status if status.is_active() => Some((DownloadStatus::Cancelled, None)),
            _ => None,
        };
    }
    let status = match (job, error) {
        (_, ShioError::PasswordRequired) => DownloadStatus::PasswordRequired,
        (Job::Extract, _) | (_, ShioError::Extract(_)) => DownloadStatus::ExtractError,
        _ => DownloadStatus::Error,
    };
    Some((status, Some(error.short_label())))
}

fn restore(db: &Database) -> Result<(Rows, VecDeque<DownloadId>)> {
    let mut rows = Rows::default();
    let mut queue = VecDeque::new();
    let mut interrupted = Vec::new();
    let mut settled = Vec::new();
    let mut active_hashes = HashSet::new();
    for mut download in db.downloads()? {
        if let Some(torrent) = download.torrent()
            && !download.status.is_terminal()
            && !active_hashes.insert(torrent.info_hash)
        {
            return Err(ShioError::Other(
                "database contains duplicate torrent info hashes".into(),
            ));
        }
        match download.status {
            DownloadStatus::Pending
            | DownloadStatus::Queued
            | DownloadStatus::Starting
            | DownloadStatus::Downloading
            | DownloadStatus::FetchingMetadata
            | DownloadStatus::Seeding => {
                download.status = DownloadStatus::Queued;
                queue.push_back(download.id);
            },
            DownloadStatus::Extracting => {
                download.status = DownloadStatus::ExtractError;
                download.error_message = Some("extraction interrupted".into());
                remove_stale_extraction(&download.file_dir());
                interrupted.push(snapshot(&download));
            },
            _ => {},
        }
        // Covers a crash between the completion commit and the staging removal.
        if download.kind.is_http()
            && download.status.is_downloaded()
            && crate::worker::settle_publication(&mut download)
        {
            settled.push(snapshot(&download));
        }
        rows.downloads.insert(download.id, download);
    }
    db.update_downloads(&interrupted)?;
    db.update_downloads(&settled)?;
    settled.iter().for_each(remove_settled_staging);
    for mut package in db.packages()? {
        if package.extract_state == PackageExtractState::Extracting {
            package.extract_state = PackageExtractState::Error;
            package.error_message = Some("extraction interrupted".into());
            remove_stale_extraction(&package.folder_path());
            db.update_package(&package)?;
        }
        rows.packages.insert(package.id, package);
    }
    Ok((rows, queue))
}

fn remove_settled_staging(download: &Download) {
    if let Err(error) = delete::remove_http_staging(download) {
        tracing::warn!(download = %download.id, "completed download left HTTP staging behind: {error}");
    }
}

fn remove_stale_extraction(dir: &Path) {
    if let Err(error) = crate::extract::remove_stale_staging(dir) {
        tracing::warn!(path = %dir.display(), "failed to remove interrupted extraction: {error}");
    }
}

fn payload_paths(download: &Download) -> Vec<PathBuf> {
    match &download.kind {
        DownloadKind::Http(_) => vec![download.file_path()],
        DownloadKind::Torrent(torrent) => torrent
            .files
            .iter()
            .filter(|file| file.selected)
            .map(|file| download.save_path.join(&file.path))
            .collect(),
    }
}

fn staging_root(download: &Download) -> PathBuf {
    match &download.kind {
        DownloadKind::Http(_) => crate::worker::http_staging_root(download),
        DownloadKind::Torrent(_) => crate::torrent::staging_root(download),
    }
}

fn overlaps(left: &[String], right: &[String]) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

// Case-folded, so `A/b` and `a/B` collide as they do on Windows and macOS.
fn path_key(path: &Path) -> Result<Vec<String>> {
    use std::path::Component;
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(
                value
                    .to_str()
                    .map(str::to_lowercase)
                    .ok_or_else(|| ShioError::Other("destination path is not valid UTF-8".into())),
            ),
            Component::Prefix(prefix) => {
                Some(Ok(prefix.as_os_str().to_string_lossy().to_lowercase()))
            },
            Component::RootDir | Component::CurDir => None,
            Component::ParentDir => Some(Err(ShioError::Other(
                "destination path contains a parent component".into(),
            ))),
        })
        .collect()
}

fn numbered(name: &str, n: u32) -> String {
    match (n, name.rsplit_once('.')) {
        (0, _) => name.to_owned(),
        (_, Some((stem, extension))) if !stem.is_empty() => format!("{stem} ({n}).{extension}"),
        _ => format!("{name} ({n})"),
    }
}

pub(crate) fn secure_private_directory(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(ShioError::Other(
                "data directory is not a real directory".into(),
            ));
        }
        if metadata.permissions().mode() & 0o777 != 0o700 {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(dir: &Path) -> (DownloadEngine, ProgressStream) {
        let (engine, _commands, events) = DownloadEngine::with_database(
            AppConfig::default(),
            dir.to_path_buf(),
            Database::in_memory().unwrap(),
        )
        .unwrap();
        (engine, events)
    }

    fn add(engine: &mut DownloadEngine, request: HttpDownloadRequest) -> DownloadId {
        let id = request.id();
        engine.add_http(request).unwrap();
        id
    }

    #[test]
    fn worker_exits_resolve_against_the_committed_status() {
        use DownloadStatus::{
            Cancelled, Completed, Downloading, Error, ExtractError, Extracting, Paused, Queued,
            Seeding,
        };
        let ok = Ok(());
        let cancelled = Err(ShioError::Cancelled);
        let failed = Err(ShioError::Other("reset".into()));
        let cancelled_extract = Some((ExtractError, Some("extraction cancelled".into())));
        let cases = [
            (Paused, &cancelled, true, false, None),
            (Queued, &cancelled, true, false, None),
            (Completed, &cancelled, true, false, None),
            (Downloading, &failed, true, false, Some((Cancelled, None))),
            (Seeding, &cancelled, true, true, Some((Queued, None))),
            (Extracting, &cancelled, true, false, cancelled_extract),
            (
                Downloading,
                &failed,
                false,
                false,
                Some((Error, Some("reset".into()))),
            ),
            (Paused, &ok, true, false, Some((Completed, None))),
        ];
        for (current, result, cancelled, shutdown, expected) in cases {
            assert_eq!(
                exit_status(Job::Download, current, result, cancelled, shutdown),
                expected,
                "{current:?}"
            );
        }
    }

    #[tokio::test]
    async fn removing_with_files_keeps_files_shio_never_published() {
        let dir = tempfile::tempdir().unwrap();
        let (mut engine, _events) = engine(dir.path());
        let id = add(
            &mut engine,
            HttpDownloadRequest::new(
                "https://example.com/report.pdf".into(),
                dir.path().to_path_buf(),
            ),
        );
        let foreign = dir.path().join("report.pdf");
        std::fs::write(&foreign, b"user file").unwrap();

        engine.remove(id, true).await.unwrap();

        assert_eq!(std::fs::read(foreign).unwrap(), b"user file");
        assert!(engine.ctx.db.get(id).unwrap().is_none());
    }

    #[tokio::test]
    async fn server_names_are_numbered_around_reserved_names() {
        let dir = tempfile::tempdir().unwrap();
        let (mut engine, _events) = engine(dir.path());
        let save = dir.path().to_path_buf();
        add(
            &mut engine,
            HttpDownloadRequest::new("https://example.com/movie.mkv".into(), save.clone()),
        );
        let id = add(
            &mut engine,
            HttpDownloadRequest::new("https://example.com/get.php".into(), save),
        );

        let claimed = engine.claim_filename(id, "movie.mkv").unwrap();

        assert_eq!(claimed, "movie (1).mkv");
        assert_eq!(engine.ctx.db.get(id).unwrap().unwrap().filename, claimed);
    }

    #[test]
    fn restart_requeues_interrupted_rows_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::in_memory().unwrap();
        let start = chrono::Utc::now();
        let statuses = [
            DownloadStatus::Downloading,
            DownloadStatus::Seeding,
            DownloadStatus::Completed,
            DownloadStatus::Extracting,
        ];
        let rows: Vec<Download> = statuses
            .into_iter()
            .zip(0..)
            .map(|(status, offset)| {
                let mut download = Download::new(
                    format!("https://example.com/{offset}.bin"),
                    dir.path().to_path_buf(),
                );
                download.status = status;
                download.created_at = start + chrono::Duration::seconds(offset);
                download
            })
            .collect();
        for row in rows.iter().rev() {
            db.insert_download(row).unwrap();
        }
        let ids: Vec<DownloadId> = rows.iter().map(|row| row.id).collect();

        let (rows, queue) = restore(&db).unwrap();

        assert_eq!(queue, [ids[0], ids[1]]);
        assert_eq!(rows.downloads[&ids[2]].status, DownloadStatus::Completed);
        assert_eq!(
            db.get(ids[3]).unwrap().unwrap().status,
            DownloadStatus::ExtractError
        );
    }

    #[test]
    fn restart_settles_a_completed_download_without_losing_ownership() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::in_memory().unwrap();
        let mut download = Download::new(
            "https://example.com/done.bin".into(),
            dir.path().to_path_buf(),
        );
        download.status = DownloadStatus::Completed;
        db.insert_download(&download).unwrap();
        let staging = crate::worker::http_staging_root(&download);
        std::fs::create_dir(&staging).unwrap();
        std::fs::write(staging.join("owner"), download.id.to_string()).unwrap();
        std::fs::write(staging.join("resume.json"), br#"{"published_size":7}"#).unwrap();
        std::fs::write(download.file_path(), b"payload").unwrap();

        let (rows, _) = restore(&db).unwrap();

        assert!(!staging.exists());
        let row = &rows.downloads[&download.id];
        assert_eq!(db.get(download.id).unwrap().unwrap().total_size, Some(7));
        assert_eq!(
            crate::worker::published_file(row),
            Some(download.file_path())
        );
        std::fs::write(download.file_path(), b"a user's file").unwrap();
        assert_eq!(crate::worker::published_file(row), None);
    }

    #[tokio::test]
    async fn a_lagging_stream_resyncs_instead_of_dropping_events() {
        let dir = tempfile::tempdir().unwrap();
        let (engine, mut events) = engine(dir.path());
        assert!(matches!(
            events.recv().await,
            Some(ProgressEvent::Resync { .. })
        ));

        for _ in 0..=EVENT_CAPACITY {
            let _ = engine.ctx.events.send(ProgressEvent::Removed {
                downloads: Vec::new(),
                packages: Vec::new(),
            });
        }

        assert!(matches!(
            events.recv().await,
            Some(ProgressEvent::Resync { .. })
        ));
        assert!(matches!(
            events.resubscribe().recv().await,
            Some(ProgressEvent::Resync { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn data_directory_becomes_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();

        secure_private_directory(dir.path()).unwrap();

        let mode = std::fs::metadata(dir.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}
