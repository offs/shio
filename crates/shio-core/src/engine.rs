mod command;
mod db_writer;
mod slot;

use self::command::ack;
use self::db_writer::{DbWrite, TorrentRuntimeWrite, log_db_critical, spawn_db_writer};
use self::slot::ActiveSlot;
use crate::config::AppConfig;
use crate::db::Database;
use crate::error::ShioError;
use crate::queue::DownloadQueue;
use crate::types::{
    ArchivePackage, Download, DownloadId, DownloadKind, DownloadProgress, DownloadStatus,
    HttpPreview, HttpPreviewResult, HttpPreviewState, HttpState, MagnetPreviewResult,
    PackageExtractState, PackageId, PackageItem, ProgressDetail, TorrentFile, TorrentSource,
};
use crate::worker::{DownloadWorker, build_client};
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use tokio::sync::{Notify, OnceCell, broadcast, mpsc};
use tokio_util::sync::CancellationToken;

pub use self::command::EngineCommand;

const PROGRESS_BROADCAST_CAPACITY: usize = 256;
const COMMAND_CHANNEL_CAPACITY: usize = 128;
const WORKER_PROGRESS_CAPACITY: usize = 64;
const DB_WRITER_CAPACITY: usize = 256;
const PROGRESS_DB_FLUSH_MS: u128 = 1000;
const WORKER_SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const DB_WRITER_SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Clone)]
pub struct HttpDownloadRequest {
    id: DownloadId,
    url: String,
    save_path: std::path::PathBuf,
    filename: Option<String>,
    segments: Option<u8>,
    subfolder: Option<String>,
    auto_extract: bool,
}

#[derive(Debug, Clone)]
pub struct HttpArchivePartRequest {
    url: String,
    filename: String,
    part_number: u32,
    segments: u8,
}

impl HttpArchivePartRequest {
    pub fn new(
        url: String,
        filename: String,
        part_number: u32,
        segments: u8,
    ) -> crate::Result<Self> {
        validate_http_url(&url)?;
        crate::path::validate_leaf_filename(&filename)?;
        if part_number == 0 {
            return Err(ShioError::Other(
                "archive part number must be positive".into(),
            ));
        }
        Ok(Self {
            url,
            filename,
            part_number,
            segments,
        })
    }
}

#[derive(Debug, Clone)]
pub struct HttpArchiveSetRequest {
    package: ArchivePackage,
    parts: Vec<HttpArchivePartRequest>,
}

impl HttpArchiveSetRequest {
    pub fn new(
        name: String,
        save_path: std::path::PathBuf,
        mut parts: Vec<HttpArchivePartRequest>,
        auto_extract: bool,
    ) -> crate::Result<Self> {
        crate::path::validate_leaf_filename(&name)?;
        crate::path::path_to_utf8("packages.save_path", &save_path)?;
        if parts.len() < 2 {
            return Err(ShioError::Other(
                "archive set requires at least two parts".into(),
            ));
        }
        parts.sort_by_key(|part| part.part_number);
        for (index, part) in parts.iter().enumerate() {
            let expected = u32::try_from(index + 1)
                .map_err(|_| ShioError::Other("too many archive parts".into()))?;
            if part.part_number != expected {
                return Err(ShioError::Other(format!(
                    "missing archive part {expected:02}"
                )));
            }
            if index > 0 && parts[index - 1].part_number == part.part_number {
                return Err(ShioError::Other(format!(
                    "duplicate archive part {}",
                    part.part_number
                )));
            }
        }
        let mut seen = HashMap::<String, String>::new();
        for part in &parts {
            let network_url = crate::worker::url_without_fragment(&part.url);
            if let Some(existing) = seen.insert(network_url, part.filename.clone())
                && existing != part.filename
            {
                return Err(ShioError::Other(
                    "multiple filenames point to the same URL".into(),
                ));
            }
        }
        Ok(Self {
            package: ArchivePackage::new(name, save_path, auto_extract),
            parts,
        })
    }

    pub const fn package(&self) -> &ArchivePackage {
        &self.package
    }

    pub fn into_package_downloads(mut self) -> crate::Result<(ArchivePackage, Vec<Download>)> {
        let mut downloads = Vec::with_capacity(self.parts.len());
        for (position, part) in self.parts.into_iter().enumerate() {
            let position = u32::try_from(position)
                .map_err(|_| ShioError::Other("too many archive parts".into()))?;
            let mut request = HttpDownloadRequest::new(part.url, self.package.save_path.clone())
                .with_filename(part.filename)
                .with_segments(part.segments)
                .with_subfolder(Some(self.package.name.clone()));
            // Package extraction owns archive-set extraction.
            request.auto_extract = false;
            let download = request.into_download();
            self.package.items.push(PackageItem {
                download_id: download.id,
                position,
                part_number: part.part_number,
            });
            downloads.push(download);
        }
        Ok((self.package, downloads))
    }
}

impl std::fmt::Debug for HttpDownloadRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpDownloadRequest")
            .field("id", &self.id)
            .field("url", &"<redacted>")
            .field("save_path", &self.save_path)
            .field("filename", &self.filename)
            .field("segments", &self.segments)
            .field("subfolder", &self.subfolder)
            .field("auto_extract", &self.auto_extract)
            .finish()
    }
}

impl HttpDownloadRequest {
    pub fn new(url: String, save_path: std::path::PathBuf) -> Self {
        Self {
            id: DownloadId::new(),
            url,
            save_path,
            filename: None,
            segments: None,
            subfolder: None,
            auto_extract: false,
        }
    }

    pub const fn id(&self) -> DownloadId {
        self.id
    }

    pub const fn with_id(mut self, id: DownloadId) -> Self {
        self.id = id;
        self
    }

    pub fn with_filename(mut self, filename: String) -> Self {
        self.filename = Some(filename);
        self
    }

    pub const fn with_segments(mut self, segments: u8) -> Self {
        self.segments = Some(segments);
        self
    }

    pub fn with_subfolder(mut self, subfolder: Option<String>) -> Self {
        self.subfolder = subfolder;
        self
    }

    pub const fn enable_auto_extract(mut self) -> Self {
        self.auto_extract = true;
        self
    }

    pub fn into_download(self) -> Download {
        let mut state = HttpState::new(self.url.clone());
        if let Some(segments) = self.segments {
            state.segments = segments;
        }
        state.subfolder = self.subfolder;
        state.auto_extract = self.auto_extract;
        Download {
            id: self.id,
            filename: self
                .filename
                .unwrap_or_else(|| crate::filename::extract_filename(&self.url, None)),
            save_path: self.save_path,
            total_size: None,
            downloaded: 0,
            status: DownloadStatus::Pending,
            priority: 0,
            speed: 0,
            avg_speed: 0,
            error_message: None,
            created_at: chrono::Utc::now(),
            started_at: None,
            completed_at: None,
            retry_count: 0,
            max_retries: 3,
            pinned: false,
            kind: DownloadKind::Http(state),
        }
    }
}

#[derive(Clone)]
pub struct TorrentDownloadRequest {
    id: DownloadId,
    source: TorrentSource,
    save_path: std::path::PathBuf,
    filename: Option<String>,
    total_size: Option<u64>,
    auto_extract: bool,
    is_private: Option<bool>,
    files: Option<Vec<TorrentFile>>,
    trackers: Option<Vec<String>>,
    metadata_bytes: Option<Vec<u8>>,
}

impl std::fmt::Debug for TorrentDownloadRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TorrentDownloadRequest")
            .field("id", &self.id)
            .field("source", &"<redacted>")
            .field("save_path", &self.save_path)
            .field("filename", &self.filename)
            .field("total_size", &self.total_size)
            .field("auto_extract", &self.auto_extract)
            .field("is_private", &self.is_private)
            .field("files", &self.files.as_ref().map(Vec::len))
            .field("trackers", &self.trackers.as_ref().map(Vec::len))
            .field(
                "metadata_bytes",
                &self.metadata_bytes.as_ref().map(Vec::len),
            )
            .finish()
    }
}

impl TorrentDownloadRequest {
    pub fn new(source: TorrentSource, save_path: std::path::PathBuf) -> Self {
        Self {
            id: DownloadId::new(),
            source,
            save_path,
            filename: None,
            total_size: None,
            auto_extract: false,
            is_private: None,
            files: None,
            trackers: None,
            metadata_bytes: None,
        }
    }

    pub const fn id(&self) -> DownloadId {
        self.id
    }

    pub const fn with_id(mut self, id: DownloadId) -> Self {
        self.id = id;
        self
    }

    pub fn with_filename(mut self, filename: String) -> Self {
        self.filename = Some(filename);
        self
    }

    pub const fn with_total_size(mut self, total_size: u64) -> Self {
        self.total_size = Some(total_size);
        self
    }

    pub const fn enable_auto_extract(mut self) -> Self {
        self.auto_extract = true;
        self
    }

    pub const fn with_private(mut self, is_private: bool) -> Self {
        self.is_private = Some(is_private);
        self
    }

    pub fn with_files(mut self, files: Vec<TorrentFile>) -> Self {
        self.files = Some(files);
        self
    }

    pub fn with_trackers(mut self, trackers: Vec<String>) -> Self {
        self.trackers = Some(trackers);
        self
    }

    pub fn with_metadata_bytes(mut self, metadata_bytes: Option<Vec<u8>>) -> Self {
        self.metadata_bytes = metadata_bytes;
        self
    }

    pub fn into_download(self) -> crate::Result<Download> {
        let mut download = Download::try_from_torrent(self.source, self.save_path)?;
        download.id = self.id;
        if let Some(filename) = self.filename {
            download.filename = filename;
        }
        if let Some(total_size) = self.total_size {
            download.total_size = Some(total_size);
        }
        if let Some(torrent) = download.torrent_mut() {
            torrent.auto_extract = self.auto_extract;
            if let Some(is_private) = self.is_private {
                torrent.is_private = is_private;
            }
            if let Some(files) = self.files {
                torrent.files = files;
            }
            if let Some(trackers) = self.trackers {
                torrent.trackers = trackers;
            }
            torrent.metadata_bytes = self.metadata_bytes;
        }
        Ok(download)
    }
}

fn validate_http_url(url: &str) -> crate::Result<()> {
    let parsed = url::Url::parse(url)
        .map_err(|error| ShioError::Other(format!("invalid http url: {error}")))?;
    if matches!(parsed.scheme(), "http" | "https") && parsed.has_host() {
        return Ok(());
    }
    Err(ShioError::Other(
        "http url must use http or https and include a host".into(),
    ))
}

fn validate_download_http_url(download: &Download) -> crate::Result<()> {
    if let Some(http) = download.http() {
        validate_http_url(&http.url)?;
    }
    Ok(())
}

struct EngineState {
    downloads: RwLock<HashMap<DownloadId, Download>>,
    packages: RwLock<HashMap<PackageId, ArchivePackage>>,
    queue: Mutex<DownloadQueue>,
    active_count: AtomicU8,
    config: RwLock<AppConfig>,
    shutdown_token: CancellationToken,
    active_cancels: Mutex<HashMap<DownloadId, Arc<CancellationToken>>>,
    active_http_previews: Mutex<HashMap<u64, Arc<CancellationToken>>>,
    cancelled_http_previews: Mutex<HashSet<u64>>,
    active_workers: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    slot_notify: Notify,
    torrent_session: OnceCell<Arc<librqbit::Session>>,
}

impl std::fmt::Debug for EngineState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineState")
            .field("active_count", &self.active_count)
            .finish_non_exhaustive()
    }
}

pub struct DownloadEngine {
    state: Arc<EngineState>,
    db: Arc<Database>,
    http: reqwest::Client,
    db_writer_tx: Option<mpsc::Sender<DbWrite>>,
    db_writer_handle: Option<tokio::task::JoinHandle<()>>,
    progress_tx: broadcast::Sender<DownloadProgress>,
    command_tx: mpsc::Sender<EngineCommand>,
    command_rx: mpsc::Receiver<EngineCommand>,
}

impl std::fmt::Debug for DownloadEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DownloadEngine").finish_non_exhaustive()
    }
}

pub struct ProgressStream {
    inner: broadcast::Receiver<DownloadProgress>,
}

impl ProgressStream {
    pub fn resubscribe(&self) -> Self {
        Self {
            inner: self.inner.resubscribe(),
        }
    }

    pub async fn recv(&mut self) -> Option<DownloadProgress> {
        loop {
            match self.inner.recv().await {
                Ok(p) => return Some(p),
                Err(broadcast::error::RecvError::Closed) => return None,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("progress stream lagged by {n}");
                },
            }
        }
    }
}

impl std::fmt::Debug for ProgressStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgressStream").finish_non_exhaustive()
    }
}

#[derive(Clone)]
struct EngineWorkerHandle {
    state: Arc<EngineState>,
}

impl EngineWorkerHandle {
    async fn ensure_torrent_session(&self) -> crate::Result<Arc<librqbit::Session>> {
        let state = self.state.clone();
        state
            .torrent_session
            .get_or_try_init(|| async {
                let (port, dht, upnp) = {
                    let cfg = state.config.read();
                    (cfg.torrent.listen_port, cfg.torrent.dht, cfg.torrent.upnp)
                };
                let data_dir = crate::config::AppConfig::data_dir();
                crate::torrent::open_session(&data_dir, port, dht, upnp).await
            })
            .await
            .cloned()
    }
}

impl DownloadEngine {
    pub fn new(config: AppConfig, db_path: &Path) -> crate::Result<(Self, ProgressStream)> {
        let db = Database::open(db_path)?;
        Self::with_database(config, db)
    }

    fn with_database(config: AppConfig, db: Database) -> crate::Result<(Self, ProgressStream)> {
        let http = build_client(&config)?;
        let (progress_tx, progress_rx) = broadcast::channel(PROGRESS_BROADCAST_CAPACITY);
        let (command_tx, command_rx) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);
        let (db_writer_tx, db_writer_rx) = mpsc::channel(DB_WRITER_CAPACITY);

        let state = Arc::new(EngineState {
            downloads: RwLock::new(HashMap::new()),
            packages: RwLock::new(HashMap::new()),
            queue: Mutex::new(DownloadQueue::new()),
            active_count: AtomicU8::new(0),
            config: RwLock::new(config),
            shutdown_token: CancellationToken::new(),
            active_cancels: Mutex::new(HashMap::new()),
            active_http_previews: Mutex::new(HashMap::new()),
            cancelled_http_previews: Mutex::new(HashSet::new()),
            active_workers: Mutex::new(Vec::new()),
            slot_notify: Notify::new(),
            torrent_session: OnceCell::new(),
        });

        let restored = db.all()?;
        let (total, requeued) = restore_downloads(&state, restored);
        for package in db.packages()? {
            state.packages.write().insert(package.id, package);
        }
        if total > 0 {
            tracing::debug!("restored {total} downloads ({requeued} re-queued)");
        }

        let db = Arc::new(db);
        let db_writer_handle = spawn_db_writer(db.clone(), db_writer_rx);

        Ok((
            Self {
                state,
                db,
                http,
                db_writer_tx: Some(db_writer_tx),
                db_writer_handle: Some(db_writer_handle),
                progress_tx,
                command_tx,
                command_rx,
            },
            ProgressStream { inner: progress_rx },
        ))
    }

    pub fn command_sender(&self) -> mpsc::Sender<EngineCommand> {
        self.command_tx.clone()
    }

    pub fn downloads(&self) -> Vec<Download> {
        self.state.downloads.read().values().cloned().collect()
    }

    pub fn packages(&self) -> Vec<ArchivePackage> {
        self.state.packages.read().values().cloned().collect()
    }

    pub async fn run(mut self) {
        let mut shutdown_ack = None;
        loop {
            self.try_start_queued();

            tokio::select! {
                Some(cmd) = self.command_rx.recv() => {
                    if let EngineCommand::Shutdown { ack } = cmd {
                        shutdown_ack = Some(ack);
                        break;
                    }
                    self.handle_command(cmd).await;
                }
                () = self.state.slot_notify.notified() => {}
                else => break,
            }
        }

        self.shutdown().await;
        if let Some(ack) = shutdown_ack {
            ack.notify_waiters();
        }
    }

    async fn handle_command(&self, cmd: EngineCommand) {
        match cmd {
            EngineCommand::AddHttp { request, reply } => {
                ack(reply, self.add(request.into_download()));
            },
            EngineCommand::AddHttpArchiveSet { request, reply } => {
                ack(reply, self.add_archive_set(request));
            },
            EngineCommand::AddTorrentPrepared { request, reply } => {
                ack(reply, self.add_torrent_prepared(request));
            },
            EngineCommand::Pause { id, reply } => ack(reply, self.pause(id).await),
            EngineCommand::Resume { id, reply } => ack(reply, self.resume(id)),
            EngineCommand::Cancel { id, reply } => ack(reply, self.cancel(id).await),
            EngineCommand::Remove {
                id,
                delete_files,
                reply,
            } => ack(reply, self.remove(id, delete_files).await),
            EngineCommand::Retry { id, reply } => ack(reply, self.retry(id)),
            EngineCommand::RetryExtract {
                id,
                password,
                reply,
            } => ack(reply, self.retry_extract(id, password)),
            EngineCommand::PauseAll { reply } => ack(reply, self.pause_all().await),
            EngineCommand::ResumeAll { reply } => ack(reply, self.resume_all()),
            EngineCommand::SetSpeedLimit(limit) => {
                self.state.config.write().speed_limit = limit;
            },
            EngineCommand::SetMaxConcurrent(max) => {
                self.state.config.write().max_concurrent = max.clamp(1, 20);
                self.state.slot_notify.notify_waiters();
            },
            EngineCommand::SetTorrentConfig(config) => {
                let session_exists = self.state.torrent_session.get().is_some();
                self.state.config.write().torrent = config;
                if session_exists {
                    tracing::info!(
                        "torrent port, DHT, and UPnP changes apply after the torrent session restarts"
                    );
                }
            },
            EngineCommand::SetPin { id, pinned, reply } => ack(reply, self.set_pin(id, pinned)),
            EngineCommand::UpdateMetadata {
                id,
                filename,
                save_path,
                reply,
            } => ack(reply, self.update_metadata(id, &filename, &save_path)),
            EngineCommand::AddTorrent {
                source,
                save_path,
                start_paused,
                auto_extract,
                reply,
            } => {
                ack(
                    reply,
                    self.add_torrent(source, save_path, start_paused, auto_extract),
                );
            },
            EngineCommand::ResolveMagnetPreview {
                request_id,
                magnet,
                reply,
            } => self.resolve_magnet_preview(request_id, magnet, reply),
            EngineCommand::ResolveHttpPreview {
                request_id,
                url,
                reply,
            } => self.resolve_http_preview(request_id, url, reply),
            EngineCommand::CancelHttpPreview { request_id } => {
                self.cancel_http_preview(request_id);
            },
            EngineCommand::ForceRecheck { id, reply } => ack(reply, self.force_recheck(id).await),
            EngineCommand::StopSeeding { id, reply } => ack(reply, self.stop_seeding(id).await),
            EngineCommand::Shutdown { .. } => {},
        }
    }

    fn add(&self, mut download: Download) -> crate::Result<()> {
        validate_download_http_url(&download)?;
        validate_download_paths(&download)?;
        download.status = DownloadStatus::Queued;
        let id = download.id;
        let priority = download.priority;
        self.db.insert_download(&download)?;
        self.state.downloads.write().insert(id, download);
        self.state.queue.lock().push(id, priority);
        Ok(())
    }

    fn add_archive_set(&self, request: HttpArchiveSetRequest) -> crate::Result<()> {
        let (mut package, mut downloads) = request.into_package_downloads()?;
        for download in &downloads {
            validate_download_http_url(download)?;
            validate_download_paths(download)?;
        }
        for download in &mut downloads {
            download.status = DownloadStatus::Queued;
        }
        package.items.sort_by_key(|item| item.position);
        self.db.insert_archive_package(&package, &downloads)?;
        {
            let mut state_downloads = self.state.downloads.write();
            for download in downloads {
                state_downloads.insert(download.id, download);
            }
        }
        {
            let mut queue = self.state.queue.lock();
            for item in &package.items {
                queue.push(item.download_id, 0);
            }
        }
        self.state.packages.write().insert(package.id, package);
        Ok(())
    }

    fn add_torrent_prepared(&self, request: TorrentDownloadRequest) -> crate::Result<()> {
        self.add(request.into_download()?)
    }

    fn add_torrent(
        &self,
        source: crate::types::TorrentSource,
        save_path: std::path::PathBuf,
        start_paused: bool,
        auto_extract: bool,
    ) -> crate::Result<()> {
        let mut download = crate::types::Download::try_from_torrent(source, save_path)?;
        validate_download_paths(&download)?;
        if let Some(torrent) = download.torrent_mut() {
            torrent.auto_extract = auto_extract;
        }
        download.status = if start_paused {
            DownloadStatus::Paused
        } else {
            DownloadStatus::Queued
        };
        let id = download.id;
        let priority = download.priority;

        self.db.insert_download(&download)?;
        self.state.downloads.write().insert(id, download);
        if !start_paused {
            self.state.queue.lock().push(id, priority);
        }
        Ok(())
    }

    fn resolve_magnet_preview(
        &self,
        request_id: u64,
        magnet: String,
        reply: mpsc::Sender<MagnetPreviewResult>,
    ) {
        const PREVIEW_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(2);

        let worker = self.clone_for_worker();
        tokio::spawn(async move {
            let result = match worker.ensure_torrent_session().await {
                Ok(session) => tokio::time::timeout(
                    PREVIEW_TIMEOUT,
                    crate::torrent::resolve_magnet_preview(&session, &magnet),
                )
                .await
                .map_or_else(
                    |_| Err("metadata lookup timed out".to_string()),
                    |result| result.map_err(|error| error.short_label()),
                ),
                Err(error) => Err(error.short_label()),
            };

            let _ = reply
                .send(MagnetPreviewResult {
                    request_id,
                    magnet,
                    result,
                })
                .await;
        });
    }

    fn resolve_http_preview(
        &self,
        request_id: u64,
        url: String,
        reply: mpsc::Sender<HttpPreviewResult>,
    ) {
        const PREVIEW_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

        let client = self.http.clone();
        let cancel = Arc::new(self.state.shutdown_token.child_token());
        if self
            .state
            .cancelled_http_previews
            .lock()
            .remove(&request_id)
        {
            return;
        }
        self.state
            .active_http_previews
            .lock()
            .insert(request_id, cancel.clone());
        let state = self.state.clone();
        let handle = tokio::spawn(async move {
            let preview = async {
                tokio::time::timeout(PREVIEW_TIMEOUT, async {
                    let network_url = crate::worker::url_without_fragment(&url);
                    let probe = crate::probe::probe_server(&client, &network_url, &[]).await?;
                    let filename =
                        fragment_filename(&url).unwrap_or_else(|| probe.response_filename.clone());

                    Ok(HttpPreview {
                        filename,
                        total_size: probe.content_length,
                        content_type: probe.content_type,
                        accept_ranges: probe.accept_ranges,
                    })
                })
                .await
            };

            let result = tokio::select! {
                () = cancel.cancelled() => {
                    let mut previews = state.active_http_previews.lock();
                    if previews
                        .get(&request_id)
                        .is_some_and(|active| Arc::ptr_eq(active, &cancel))
                    {
                        previews.remove(&request_id);
                    }
                    return;
                },
                preview = preview => match preview {
                    Ok(Ok(preview)) => HttpPreviewResult {
                        request_id,
                        state: HttpPreviewState::Ready(preview),
                    },
                    Ok(Err(error)) => http_preview_error_result(request_id, error),
                    Err(_) => HttpPreviewResult {
                        request_id,
                        state: HttpPreviewState::Error {
                            message: "preview timed out".to_string(),
                        },
                    },
                },
            };

            {
                let mut previews = state.active_http_previews.lock();
                if previews
                    .get(&request_id)
                    .is_some_and(|active| Arc::ptr_eq(active, &cancel))
                {
                    previews.remove(&request_id);
                }
            }

            if let Err(e) = reply.send(result).await {
                tracing::debug!("http preview reply dropped: {e}");
            }
        });
        self.state.active_workers.lock().push(handle);
    }

    fn cancel_http_preview(&self, request_id: u64) {
        self.state.cancelled_http_previews.lock().insert(request_id);
        let cancel = self.state.active_http_previews.lock().remove(&request_id);
        if let Some(cancel) = cancel {
            cancel.cancel();
        }
    }

    async fn force_recheck(&self, id: DownloadId) -> crate::Result<()> {
        if !self
            .state
            .downloads
            .read()
            .get(&id)
            .is_some_and(|download| download.kind.is_torrent())
        {
            return Err(ShioError::Other("download is not a torrent".into()));
        }

        self.state.queue.lock().remove(&id);
        self.delete_torrent_session(id, false).await;

        let cancel = {
            let active_cancels = self.state.active_cancels.lock();
            active_cancels.get(&id).cloned()
        };
        if let Some(cancel) = cancel {
            cancel.cancel();
            loop {
                let still_active = {
                    let active_cancels = self.state.active_cancels.lock();
                    active_cancels
                        .get(&id)
                        .is_some_and(|active| Arc::ptr_eq(active, &cancel))
                };
                if !still_active {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        }

        let priority = {
            let mut downloads = self.state.downloads.write();
            downloads.get_mut(&id).map(|download| {
                download.status = DownloadStatus::Queued;
                download.error_message = None;
                download.speed = 0;
                download.avg_speed = 0;
                download.priority
            })
        };

        if let Some(priority) = priority {
            self.state.queue.lock().push(id, priority);
            log_db_critical(
                "force_recheck queued",
                id,
                self.db.update_status(id, DownloadStatus::Queued, None),
            );
            Ok(())
        } else {
            Err(ShioError::Other("download not found".into()))
        }
    }

    async fn pause(&self, id: DownloadId) -> crate::Result<()> {
        if let Some(package_id) = self.package_id_for_row(id) {
            let child_ids = self.package_child_ids(package_id);
            for child_id in child_ids {
                self.pause_download(child_id).await?;
            }
            return Ok(());
        }
        self.pause_download(id).await
    }

    async fn pause_download(&self, id: DownloadId) -> crate::Result<()> {
        self.reject_extracting_mutation(id)?;
        self.state.queue.lock().remove(&id);
        self.pause_torrent_session(id).await;
        if let Some(token) = self.state.active_cancels.lock().get(&id) {
            token.cancel();
        }
        let updated = self.mutate_status(id, DownloadStatus::Paused);
        if updated {
            log_db_critical(
                "update_status paused",
                id,
                self.db.update_status(id, DownloadStatus::Paused, None),
            );
            Ok(())
        } else {
            Err(ShioError::Other("download not found".into()))
        }
    }

    fn resume(&self, id: DownloadId) -> crate::Result<()> {
        if let Some(package_id) = self.package_id_for_row(id) {
            for child_id in self.package_child_ids(package_id) {
                self.resume(child_id)?;
            }
            return Ok(());
        }
        let priority = {
            let mut downloads = self.state.downloads.write();
            downloads.get_mut(&id).and_then(|d| {
                matches!(d.status, DownloadStatus::Paused | DownloadStatus::Error).then(|| {
                    d.status = DownloadStatus::Queued;
                    d.error_message = None;
                    d.priority
                })
            })
        };
        if let Some(p) = priority {
            self.state.queue.lock().push(id, p);
            log_db_critical(
                "update_status queued",
                id,
                self.db.update_status(id, DownloadStatus::Queued, None),
            );
            Ok(())
        } else {
            Err(ShioError::Other("download is not resumable".into()))
        }
    }

    async fn cancel(&self, id: DownloadId) -> crate::Result<()> {
        if let Some(package_id) = self.package_id_for_row(id) {
            let child_ids = self.package_child_ids(package_id);
            for child_id in child_ids {
                self.cancel_download(child_id).await?;
            }
            return Ok(());
        }
        self.cancel_download(id).await
    }

    async fn cancel_download(&self, id: DownloadId) -> crate::Result<()> {
        self.reject_extracting_mutation(id)?;
        self.delete_torrent_session(id, false).await;
        if let Some(token) = self.state.active_cancels.lock().get(&id) {
            token.cancel();
        }
        self.state.queue.lock().remove(&id);
        if self.mutate_status(id, DownloadStatus::Cancelled) {
            log_db_critical(
                "update_status cancelled",
                id,
                self.db.update_status(id, DownloadStatus::Cancelled, None),
            );
            Ok(())
        } else {
            Err(ShioError::Other("download not found".into()))
        }
    }

    async fn remove(&self, id: DownloadId, delete_files: bool) -> crate::Result<()> {
        if let Some(package_id) = self.package_id_for_row(id) {
            let child_ids = self.package_child_ids(package_id);
            for child_id in child_ids {
                self.remove_download(child_id, delete_files).await?;
            }
            self.db.delete_package(package_id)?;
            self.state.packages.write().remove(&package_id);
            return Ok(());
        }
        self.remove_download(id, delete_files).await
    }

    async fn remove_download(&self, id: DownloadId, delete_files: bool) -> crate::Result<()> {
        if !self.state.downloads.read().contains_key(&id) {
            return Err(ShioError::Other("download not found".into()));
        }
        self.reject_extracting_mutation(id)?;
        self.delete_torrent_session(id, delete_files).await;
        let cancel = self.state.active_cancels.lock().remove(&id);
        if let Some(token) = cancel {
            token.cancel();
        }
        self.state.queue.lock().remove(&id);

        if delete_files {
            let file_path = self
                .state
                .downloads
                .read()
                .get(&id)
                .and_then(|download| download.kind.is_http().then(|| vec![download.file_path()]))
                .unwrap_or_default();
            let torrent_paths = self.selected_torrent_payload_paths(id);
            for path in file_path.into_iter().chain(torrent_paths) {
                if let Err(e) = tokio::fs::remove_file(&path).await
                    && e.kind() != std::io::ErrorKind::NotFound
                {
                    return Err(e.into());
                }
            }
        }

        self.state.downloads.write().remove(&id);
        log_db_critical("delete_chunks", id, self.db.delete_chunks(id));
        log_db_critical("delete", id, self.db.delete(id));
        Ok(())
    }

    fn retry(&self, id: DownloadId) -> crate::Result<()> {
        if let Some(package_id) = self.package_id_for_row(id) {
            for child_id in self.package_child_ids(package_id) {
                self.retry(child_id)?;
            }
            return Ok(());
        }
        let priority = {
            let mut downloads = self.state.downloads.write();
            downloads.get_mut(&id).and_then(|d| {
                (d.status == DownloadStatus::Error).then(|| {
                    d.retry_count += 1;
                    d.status = DownloadStatus::Queued;
                    d.error_message = None;
                    d.priority
                })
            })
        };
        if let Some(p) = priority {
            self.state.queue.lock().push(id, p);
            log_db_critical(
                "retry queued",
                id,
                self.db.update_status(id, DownloadStatus::Queued, None),
            );
            Ok(())
        } else {
            Err(ShioError::Other("download is not retryable".into()))
        }
    }

    fn retry_extract(&self, id: DownloadId, password: Option<String>) -> crate::Result<()> {
        if let Some(package_id) = self.package_id_for_row(id) {
            self.retry_package_extract(package_id, password);
            return Ok(());
        }
        let Some(db_writer) = self.db_writer_sender() else {
            return Err(ShioError::Other("db writer closed".into()));
        };
        let download = {
            let mut downloads = self.state.downloads.write();
            downloads.get_mut(&id).and_then(|d| {
                matches!(
                    d.status,
                    DownloadStatus::ExtractError | DownloadStatus::PasswordRequired
                )
                .then(|| {
                    d.status = DownloadStatus::Extracting;
                    d.error_message = None;
                    d.clone()
                })
            })
        };
        let Some(download) = download else {
            return Err(ShioError::Other("download is not extractable".into()));
        };
        log_db_critical(
            "update_status extracting",
            id,
            self.db.update_status(id, DownloadStatus::Extracting, None),
        );

        let config = self.state.config.read().clone();
        let state = self.state.clone();
        let broadcast_tx = self.progress_tx.clone();

        let handle = tokio::spawn(async move {
            let _ = broadcast_tx.send(DownloadProgress {
                id,
                downloaded: download.downloaded,
                total_size: download.total_size,
                speed: 0,
                avg_speed: 0,
                status: DownloadStatus::Extracting,
                detail: ProgressDetail::empty_http(),
                filename: None,
                torrent_snapshot: None,
            });

            let result = crate::worker::run_extract(&download, &config, password).await;
            let final_status = crate::worker::status_for_extract_result(&result);
            let err_label = result.as_ref().err().map(ShioError::short_label);

            let (downloaded, total_size) = {
                let mut downloads = state.downloads.write();
                if let Some(d) = downloads.get_mut(&id) {
                    d.status = final_status;
                    d.error_message = err_label;
                    if final_status == DownloadStatus::Completed {
                        d.completed_at = Some(chrono::Utc::now());
                    }
                    (d.downloaded, d.total_size)
                } else {
                    (0, None)
                }
            };

            let _ = broadcast_tx.send(DownloadProgress {
                id,
                downloaded,
                total_size,
                speed: 0,
                avg_speed: 0,
                status: final_status,
                detail: ProgressDetail::empty_http(),
                filename: None,
                torrent_snapshot: None,
            });

            if let Err(e) = db_writer
                .send(DbWrite::Final {
                    id,
                    status: final_status,
                    downloaded,
                    total_size,
                    torrent_runtime: None,
                })
                .await
            {
                tracing::warn!(download = %id, "final extract db write dropped: {e}");
            }
        });
        self.state.active_workers.lock().push(handle);
        Ok(())
    }

    async fn pause_all(&self) -> crate::Result<()> {
        let ids: Vec<_> = self
            .state
            .downloads
            .read()
            .iter()
            .filter(|(_, d)| {
                matches!(
                    d.status,
                    DownloadStatus::Downloading
                        | DownloadStatus::Queued
                        | DownloadStatus::Starting
                        | DownloadStatus::FetchingMetadata
                        | DownloadStatus::Seeding
                )
            })
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.pause(id).await?;
        }
        Ok(())
    }

    fn resume_all(&self) -> crate::Result<()> {
        let ids: Vec<_> = self
            .state
            .downloads
            .read()
            .iter()
            .filter(|(_, d)| d.status == DownloadStatus::Paused)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.resume(id)?;
        }
        Ok(())
    }

    fn set_pin(&self, id: DownloadId, pinned: bool) -> crate::Result<()> {
        if let Some(package_id) = self.package_id_for_row(id) {
            if let Some(package) = self.state.packages.write().get_mut(&package_id) {
                package.pinned = pinned;
                return Ok(());
            }
            return Err(ShioError::Other("package not found".into()));
        }
        if !self.state.downloads.read().contains_key(&id) {
            return Err(ShioError::Other("download not found".into()));
        }
        self.db.update_pin(id, pinned)?;
        if let Some(download) = self.state.downloads.write().get_mut(&id) {
            download.pinned = pinned;
        }
        Ok(())
    }

    fn update_metadata(
        &self,
        id: DownloadId,
        filename: &str,
        save_path: &Path,
    ) -> crate::Result<()> {
        crate::path::validate_leaf_filename(filename)?;
        crate::path::path_to_utf8("downloads.save_path", save_path)?;
        if !self.state.downloads.read().contains_key(&id) {
            return Err(ShioError::Other("download not found".into()));
        }
        self.db.update_metadata(id, filename, save_path)?;
        if let Some(download) = self.state.downloads.write().get_mut(&id) {
            filename.clone_into(&mut download.filename);
            save_path.clone_into(&mut download.save_path);
        }
        Ok(())
    }

    async fn stop_seeding(&self, id: DownloadId) -> crate::Result<()> {
        let updated = self.mutate_status(id, DownloadStatus::Completed);
        self.delete_torrent_session(id, false).await;
        if let Some(token) = self.state.active_cancels.lock().get(&id) {
            token.cancel();
        }
        if updated {
            log_db_critical(
                "update_status completed",
                id,
                self.db.update_status(id, DownloadStatus::Completed, None),
            );
            Ok(())
        } else {
            Err(ShioError::Other("download not found".into()))
        }
    }

    fn mutate_status(&self, id: DownloadId, status: DownloadStatus) -> bool {
        let mut downloads = self.state.downloads.write();
        if let Some(d) = downloads.get_mut(&id) {
            d.status = status;
            true
        } else {
            false
        }
    }

    fn reject_extracting_mutation(&self, id: DownloadId) -> crate::Result<()> {
        let is_extracting = self
            .state
            .downloads
            .read()
            .get(&id)
            .is_some_and(|download| download.status == DownloadStatus::Extracting);
        if is_extracting {
            return Err(ShioError::Other("extraction is finishing".into()));
        }
        Ok(())
    }

    fn package_id_for_row(&self, id: DownloadId) -> Option<PackageId> {
        let package_id = PackageId(id.0);
        self.state
            .packages
            .read()
            .contains_key(&package_id)
            .then_some(package_id)
    }

    fn package_child_ids(&self, id: PackageId) -> Vec<DownloadId> {
        self.state
            .packages
            .read()
            .get(&id)
            .map(|package| package.child_ids().collect())
            .unwrap_or_default()
    }

    fn retry_package_extract(&self, package_id: PackageId, password: Option<String>) {
        let target = mark_package_extracting(&self.state, package_id);
        let Some(target) = target else {
            return;
        };
        if let Err(e) =
            self.db
                .update_package_extract_state(package_id, PackageExtractState::Extracting, None)
        {
            tracing::warn!(package = %package_id, "update package extract state failed: {e}");
        }
        let config = self.state.config.read().clone();
        let state = self.state.clone();
        let db = self.db.clone();
        let broadcast = self.progress_tx.clone();
        let handle = tokio::spawn(async move {
            run_package_extract(package_id, target, config, state, db, broadcast, password).await;
        });
        self.state.active_workers.lock().push(handle);
    }

    fn try_start_queued(&self) {
        let max = self.state.config.read().max_concurrent;
        while self.state.active_count.load(Ordering::Relaxed) < max {
            let Some(id) = self.state.queue.lock().pop() else {
                break;
            };
            self.spawn_worker(id);
        }
    }

    fn spawn_worker(&self, id: DownloadId) {
        let download = {
            let mut downloads = self.state.downloads.write();
            let Some(d) = downloads.get_mut(&id) else {
                return;
            };
            if d.status != DownloadStatus::Queued {
                return;
            }
            d.status = DownloadStatus::Starting;
            d.started_at = Some(chrono::Utc::now());
            d.clone()
        };

        log_db_critical(
            "update_status starting",
            id,
            self.db.update_status(id, DownloadStatus::Starting, None),
        );
        let _ = self.progress_tx.send(DownloadProgress {
            id,
            downloaded: download.downloaded,
            total_size: download.total_size,
            speed: 0,
            avg_speed: 0,
            status: DownloadStatus::Starting,
            detail: match &download.kind {
                crate::types::DownloadKind::Http(_) => ProgressDetail::empty_http(),
                crate::types::DownloadKind::Torrent(_) => ProgressDetail::empty_torrent(),
            },
            filename: None,
            torrent_snapshot: None,
        });

        let cancel = Arc::new(CancellationToken::new());
        self.state.active_cancels.lock().insert(id, cancel.clone());

        let (worker_tx, worker_rx) = mpsc::channel(WORKER_PROGRESS_CAPACITY);
        let progress_task = self.spawn_progress_task(id, worker_rx);
        let slot = ActiveSlot::new(self.state.clone());

        match &download.kind {
            crate::types::DownloadKind::Http(_) => {
                self.spawn_http_worker(download, &cancel, worker_tx, progress_task, slot);
            },
            crate::types::DownloadKind::Torrent(_) => {
                self.spawn_torrent_worker(download, &cancel, worker_tx, progress_task, slot);
            },
        }
    }

    fn spawn_http_worker(
        &self,
        download: crate::types::Download,
        cancel: &Arc<CancellationToken>,
        worker_tx: mpsc::Sender<DownloadProgress>,
        progress_task: tokio::task::JoinHandle<()>,
        slot: ActiveSlot,
    ) {
        let id = download.id;
        let config = self.state.config.read().clone();
        let state = self.state.clone();
        let db = self.db.clone();
        let Some(db_writer) = self.db_writer_sender() else {
            tracing::warn!(download = %id, "http worker skipped: db writer closed");
            return;
        };
        let http = self.http.clone();
        let progress_broadcast = self.progress_tx.clone();
        let owned_cancel = cancel.clone();
        let worker_token = (**cancel).clone();

        let handle = tokio::spawn(async move {
            let _slot = slot;
            let result =
                DownloadWorker::run(download, &config, http, db.clone(), worker_tx, worker_token)
                    .await;
            let _ = progress_task.await;

            let current = state
                .downloads
                .read()
                .get(&id)
                .map_or(DownloadStatus::Cancelled, |d| d.status);

            let final_status = match (&result, current) {
                (Ok(s), _) => *s,
                (Err(ShioError::Cancelled), DownloadStatus::Paused) => DownloadStatus::Paused,
                (Err(ShioError::Cancelled), _) => DownloadStatus::Cancelled,
                (Err(ShioError::PasswordRequired), _) => DownloadStatus::PasswordRequired,
                (Err(ShioError::Extract(_)), _) => DownloadStatus::ExtractError,
                (Err(_), _) => DownloadStatus::Error,
            };

            let (effective_status, downloaded, total_size) = {
                let mut downloads = state.downloads.write();
                if let Some(d) = downloads.get_mut(&id) {
                    let effective_status = effective_http_final_status(d.status, final_status);
                    d.status = effective_status;
                    d.speed = 0;
                    d.avg_speed = 0;
                    if matches!(
                        effective_status,
                        DownloadStatus::Completed
                            | DownloadStatus::ExtractError
                            | DownloadStatus::PasswordRequired
                    ) {
                        d.completed_at = Some(chrono::Utc::now());
                    }
                    d.error_message = result.as_ref().err().map(ShioError::short_label);
                    (effective_status, d.downloaded, d.total_size)
                } else {
                    (final_status, 0, None)
                }
            };

            if let Err(e) = db_writer
                .send(DbWrite::Final {
                    id,
                    status: effective_status,
                    downloaded,
                    total_size,
                    torrent_runtime: None,
                })
                .await
            {
                tracing::warn!(download = %id, "final http db write dropped: {e}");
            }

            if effective_status == DownloadStatus::Completed {
                try_start_package_extract_for_child(
                    id,
                    state.clone(),
                    db.clone(),
                    config,
                    progress_broadcast.clone(),
                );
            }

            let mut cancels = state.active_cancels.lock();
            if cancels
                .get(&id)
                .is_some_and(|existing| Arc::ptr_eq(existing, &owned_cancel))
            {
                cancels.remove(&id);
            }
        });
        self.state.active_workers.lock().push(handle);
    }

    fn spawn_torrent_worker(
        &self,
        download: crate::types::Download,
        cancel: &Arc<CancellationToken>,
        worker_tx: mpsc::Sender<DownloadProgress>,
        progress_task: tokio::task::JoinHandle<()>,
        slot: ActiveSlot,
    ) {
        let id = download.id;
        let engine = self.clone_for_worker();
        let config = self.state.config.read().clone();
        let seed_policy = config.torrent.seed_policy;
        let state = self.state.clone();
        let Some(db_writer) = self.db_writer_sender() else {
            tracing::warn!(download = %id, "torrent worker skipped: db writer closed");
            return;
        };
        let progress_broadcast = self.progress_tx.clone();
        let owned_cancel = cancel.clone();
        let worker_token = (**cancel).clone();

        let handle = tokio::spawn(async move {
            let session = match engine.ensure_torrent_session().await {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(download = %id, "failed to open torrent session: {e}");
                    finalize_torrent(
                        id,
                        &state,
                        db_writer,
                        DownloadStatus::Error,
                        Some(e.short_label()),
                    )
                    .await;
                    drop(slot);
                    state.active_cancels.lock().remove(&id);
                    return;
                },
            };

            let slot_holder = Arc::new(std::sync::Mutex::new(Some(slot)));
            let slot_for_closure = slot_holder.clone();
            let on_download_phase_finished: Box<dyn FnOnce() + Send + 'static> =
                Box::new(move || {
                    let _ = slot_for_closure.lock().ok().and_then(|mut g| g.take());
                });

            let deps = crate::torrent::TorrentWorkerDeps {
                session,
                progress_tx: worker_tx,
                cancel: worker_token,
                seed_policy,
                config,
                on_download_phase_finished,
            };

            let result = crate::torrent::run_torrent(download, deps).await;
            let _ = progress_task.await;

            if let Ok(mut guard) = slot_holder.lock() {
                guard.take();
            }

            let final_status = torrent_worker_exit_status(&result);
            let err_label = result.as_ref().err().map(ShioError::short_label);

            finalize_torrent(id, &state, db_writer, final_status, err_label).await;

            let _ = progress_broadcast;

            let mut cancels = state.active_cancels.lock();
            if cancels
                .get(&id)
                .is_some_and(|existing| Arc::ptr_eq(existing, &owned_cancel))
            {
                cancels.remove(&id);
            }
        });
        self.state.active_workers.lock().push(handle);
    }

    fn clone_for_worker(&self) -> EngineWorkerHandle {
        EngineWorkerHandle {
            state: self.state.clone(),
        }
    }

    fn db_writer_sender(&self) -> Option<mpsc::Sender<DbWrite>> {
        self.db_writer_tx.clone()
    }

    async fn shutdown(&mut self) {
        self.state.shutdown_token.cancel();
        for cancel in self.state.active_cancels.lock().values() {
            cancel.cancel();
        }

        let workers = {
            let mut active_workers = self.state.active_workers.lock();
            std::mem::take(&mut *active_workers)
        };
        for mut worker in workers {
            tokio::select! {
                result = &mut worker => {
                    if let Err(e) = result {
                        tracing::warn!("worker task join failed during shutdown: {e}");
                    }
                }
                () = tokio::time::sleep(WORKER_SHUTDOWN_TIMEOUT) => {
                    worker.abort();
                    if let Err(e) = worker.await
                        && !e.is_cancelled()
                    {
                        tracing::warn!("worker task abort failed during shutdown: {e}");
                    }
                }
            }
        }

        self.db_writer_tx.take();
        if let Some(mut handle) = self.db_writer_handle.take() {
            tokio::select! {
                result = &mut handle => {
                    if let Err(e) = result {
                        tracing::warn!("db writer join failed during shutdown: {e}");
                    }
                }
                () = tokio::time::sleep(DB_WRITER_SHUTDOWN_TIMEOUT) => {
                    handle.abort();
                    if let Err(e) = handle.await
                        && !e.is_cancelled()
                    {
                        tracing::warn!("db writer abort failed during shutdown: {e}");
                    }
                }
            }
        }

        if let Err(e) = self.db.checkpoint() {
            tracing::warn!("db checkpoint failed: {e}");
        }
    }

    fn torrent_info_hash(&self, id: DownloadId) -> Option<[u8; 20]> {
        self.state
            .downloads
            .read()
            .get(&id)
            .and_then(|download| download.torrent().map(|torrent| torrent.info_hash))
    }

    fn torrent_lookup_id(info_hash: [u8; 20]) -> librqbit::api::TorrentIdOrHash {
        librqbit::api::TorrentIdOrHash::Hash(librqbit::dht::Id20::new(info_hash))
    }

    async fn pause_torrent_session(&self, id: DownloadId) {
        let Some(session) = self.state.torrent_session.get().cloned() else {
            return;
        };
        let Some(info_hash) = self.torrent_info_hash(id) else {
            return;
        };
        let handle = session.get(Self::torrent_lookup_id(info_hash));
        let Some(handle) = handle else {
            return;
        };
        if let Err(e) = session.pause(&handle).await {
            tracing::warn!(download = %id, "pause torrent session failed: {e}");
        }
    }

    async fn delete_torrent_session(&self, id: DownloadId, delete_files: bool) {
        let Some(session) = self.state.torrent_session.get().cloned() else {
            return;
        };
        let Some(info_hash) = self.torrent_info_hash(id) else {
            return;
        };
        let mode = if delete_files {
            crate::torrent::TorrentSessionStop::DeleteFiles
        } else {
            crate::torrent::TorrentSessionStop::KeepFiles
        };
        if let Err(e) = crate::torrent::stop_torrent_in_session(&session, info_hash, mode).await {
            tracing::warn!(download = %id, delete_files, "delete torrent session failed: {e}");
        }
    }

    fn selected_torrent_payload_paths(&self, id: DownloadId) -> Vec<std::path::PathBuf> {
        let downloads = self.state.downloads.read();
        let Some(download) = downloads.get(&id) else {
            return Vec::new();
        };
        let Some(torrent) = download.torrent() else {
            return Vec::new();
        };
        let selected: Vec<_> = torrent.files.iter().filter(|file| file.selected).collect();
        let files = if selected.is_empty() {
            torrent.files.iter().collect::<Vec<_>>()
        } else {
            selected
        };
        files
            .into_iter()
            .filter_map(|file| {
                if let Err(e) = crate::path::validate_relative_path(&file.path) {
                    tracing::warn!(
                        download = %id,
                        path = %file.path.display(),
                        "skip unsafe torrent payload deletion: {e}"
                    );
                    return None;
                }
                Some(download.save_path.join(&file.path))
            })
            .collect()
    }

    fn spawn_progress_task(
        &self,
        id: DownloadId,
        mut rx: mpsc::Receiver<DownloadProgress>,
    ) -> tokio::task::JoinHandle<()> {
        let state = self.state.clone();
        let db_writer = self.db_writer_sender();
        let broadcast_tx = self.progress_tx.clone();
        tokio::spawn(async move {
            let mut last_flush: Option<std::time::Instant> = None;
            while let Some(progress) = rx.recv().await {
                let metadata_write = progress
                    .filename
                    .as_ref()
                    .map(|filename| DbWrite::Metadata {
                        id,
                        filename: filename.clone(),
                        save_path: state
                            .downloads
                            .read()
                            .get(&id)
                            .map_or_else(std::path::PathBuf::new, |download| {
                                download.save_path.clone()
                            }),
                    });

                let torrent_metadata_write =
                    progress
                        .torrent_snapshot
                        .as_ref()
                        .map(|snapshot| DbWrite::TorrentMetadata {
                            id,
                            is_private: snapshot.is_private,
                            files: snapshot.files.clone(),
                            trackers: snapshot.trackers.clone(),
                        });

                {
                    let mut downloads = state.downloads.write();
                    if let Some(d) = downloads.get_mut(&id) {
                        if let Some(filename) = progress.filename.as_ref() {
                            d.filename.clone_from(filename);
                        }
                        if let Some(snapshot) = progress.torrent_snapshot.as_ref()
                            && let Some(torrent) = d.torrent_mut()
                        {
                            torrent.is_private = snapshot.is_private;
                            torrent.files.clone_from(&snapshot.files);
                            torrent.trackers.clone_from(&snapshot.trackers);
                        }
                        d.downloaded = progress.downloaded;
                        d.total_size = progress.total_size.or(d.total_size);
                        d.speed = progress.speed;
                        d.avg_speed = progress.avg_speed;
                        if let ProgressDetail::Torrent {
                            peers_connected,
                            seeders,
                            leechers,
                            uploaded,
                            upload_speed,
                            ratio,
                            seed_elapsed_secs,
                            metadata_wait_secs,
                        } = &progress.detail
                            && let Some(torrent) = d.torrent_mut()
                        {
                            torrent.peers_connected = *peers_connected;
                            torrent.seeders = *seeders;
                            torrent.leechers = *leechers;
                            torrent.uploaded = *uploaded;
                            torrent.upload_speed = *upload_speed;
                            torrent.ratio = *ratio;
                            torrent.seed_elapsed_secs = *seed_elapsed_secs;
                            torrent.metadata_wait_secs = *metadata_wait_secs;
                        }
                        if matches!(
                            d.status,
                            DownloadStatus::Starting
                                | DownloadStatus::Downloading
                                | DownloadStatus::Extracting
                                | DownloadStatus::FetchingMetadata
                                | DownloadStatus::Seeding
                        ) {
                            d.status = progress.status;
                        }
                    }
                }

                if let Some(write) = metadata_write {
                    if let Some(db_writer) = db_writer.as_ref() {
                        if let Err(e) = db_writer.send(write).await {
                            tracing::warn!(download = %id, "metadata db write dropped: {e}");
                        }
                    } else {
                        tracing::warn!(download = %id, "metadata db write dropped: db writer closed");
                    }
                }
                if let Some(write) = torrent_metadata_write {
                    if let Some(db_writer) = db_writer.as_ref() {
                        if let Err(e) = db_writer.send(write).await {
                            tracing::warn!(download = %id, "torrent metadata db write dropped: {e}");
                        }
                    } else {
                        tracing::warn!(download = %id, "torrent metadata db write dropped: db writer closed");
                    }
                }

                let should_flush = matches!(
                    progress.status,
                    DownloadStatus::Paused
                        | DownloadStatus::Cancelled
                        | DownloadStatus::Completed
                        | DownloadStatus::Error
                        | DownloadStatus::ExtractError
                        | DownloadStatus::PasswordRequired
                ) || last_flush
                    .is_none_or(|t| t.elapsed().as_millis() >= PROGRESS_DB_FLUSH_MS);
                if should_flush {
                    if let Some(db_writer) = db_writer.as_ref()
                        && let Err(e) = db_writer.try_send(DbWrite::Progress {
                            id,
                            downloaded: progress.downloaded,
                            total_size: progress.total_size,
                        })
                    {
                        tracing::debug!(download = %id, "progress db write coalesced: {e}");
                    }
                    if let ProgressDetail::Torrent {
                        uploaded,
                        ratio,
                        seed_elapsed_secs,
                        ..
                    } = &progress.detail
                        && let Some(db_writer) = db_writer.as_ref()
                        && let Err(e) = db_writer.try_send(DbWrite::TorrentRuntime {
                            id,
                            uploaded: *uploaded,
                            ratio: *ratio,
                            seed_elapsed_secs: *seed_elapsed_secs,
                        })
                    {
                        tracing::debug!(download = %id, "torrent runtime db write coalesced: {e}");
                    }
                    last_flush = Some(std::time::Instant::now());
                }

                let _ = broadcast_tx.send(progress);
            }
        })
    }
}

fn validate_download_paths(download: &Download) -> crate::Result<()> {
    crate::path::validate_leaf_filename(&download.filename)?;
    crate::path::path_to_utf8("downloads.save_path", &download.save_path)?;
    if let Some(http) = download.http()
        && let Some(subfolder) = http.subfolder.as_deref()
    {
        crate::path::validate_leaf_filename(subfolder)?;
    }
    if let Some(torrent) = download.torrent() {
        for file in &torrent.files {
            crate::path::validate_relative_path(&file.path)?;
        }
    }
    Ok(())
}

fn mark_package_extracting(state: &Arc<EngineState>, package_id: PackageId) -> Option<Download> {
    let target_id = {
        let mut packages = state.packages.write();
        let package = packages.get_mut(&package_id)?;
        if !package.auto_extract || package.extract_state != PackageExtractState::NotStarted {
            return None;
        }
        let downloads = state.downloads.read();
        if !package.items.iter().all(|item| {
            downloads
                .get(&item.download_id)
                .is_some_and(|d| d.status == DownloadStatus::Completed)
        }) {
            return None;
        }
        let target = package
            .items
            .iter()
            .min_by_key(|item| item.part_number)
            .map(|item| item.download_id)?;
        package.extract_state = PackageExtractState::Extracting;
        package.error_message = None;
        target
    };
    state.downloads.read().get(&target_id).cloned()
}

fn try_start_package_extract_for_child(
    child_id: DownloadId,
    state: Arc<EngineState>,
    db: Arc<Database>,
    config: AppConfig,
    broadcast: broadcast::Sender<DownloadProgress>,
) {
    let package_id = {
        let packages = state.packages.read();
        packages.iter().find_map(|(id, package)| {
            package
                .items
                .iter()
                .any(|item| item.download_id == child_id)
                .then_some(*id)
        })
    };
    let Some(package_id) = package_id else {
        return;
    };
    let Some(target) = mark_package_extracting(&state, package_id) else {
        return;
    };
    if let Err(e) =
        db.update_package_extract_state(package_id, PackageExtractState::Extracting, None)
    {
        tracing::warn!(package = %package_id, "update package extract state failed: {e}");
    }
    tokio::spawn(async move {
        run_package_extract(package_id, target, config, state, db, broadcast, None).await;
    });
}

async fn run_package_extract(
    package_id: PackageId,
    target: Download,
    config: AppConfig,
    state: Arc<EngineState>,
    db: Arc<Database>,
    broadcast: broadcast::Sender<DownloadProgress>,
    password: Option<String>,
) {
    let row_id = DownloadId(package_id.0);
    let _ = broadcast.send(DownloadProgress {
        id: row_id,
        downloaded: target.downloaded,
        total_size: target.total_size,
        speed: 0,
        avg_speed: 0,
        status: DownloadStatus::Extracting,
        detail: ProgressDetail::empty_http(),
        filename: None,
        torrent_snapshot: None,
    });

    let result = crate::worker::run_extract(&target, &config, password).await;
    let (extract_state, status) = match crate::worker::status_for_extract_result(&result) {
        DownloadStatus::Completed => (PackageExtractState::Completed, DownloadStatus::Completed),
        DownloadStatus::PasswordRequired => (
            PackageExtractState::PasswordRequired,
            DownloadStatus::PasswordRequired,
        ),
        _ => (PackageExtractState::Error, DownloadStatus::ExtractError),
    };
    let err_label = result.as_ref().err().map(ShioError::short_label);
    {
        let mut packages = state.packages.write();
        if let Some(package) = packages.get_mut(&package_id) {
            package.extract_state = extract_state;
            package.error_message.clone_from(&err_label);
            if extract_state == PackageExtractState::Completed {
                package.completed_at = Some(chrono::Utc::now());
            }
        }
    }
    if let Err(e) = db.update_package_extract_state(package_id, extract_state, err_label.as_deref())
    {
        tracing::warn!(package = %package_id, "update package extract state failed: {e}");
    }
    let _ = broadcast.send(DownloadProgress {
        id: row_id,
        downloaded: target.downloaded,
        total_size: target.total_size,
        speed: 0,
        avg_speed: 0,
        status,
        detail: ProgressDetail::empty_http(),
        filename: None,
        torrent_snapshot: None,
    });
}

const fn torrent_worker_exit_status(
    result: &crate::Result<crate::torrent::TorrentWorkerExit>,
) -> DownloadStatus {
    match result {
        Ok(
            crate::torrent::TorrentWorkerExit::Completed
            | crate::torrent::TorrentWorkerExit::SeedingStopped,
        ) => DownloadStatus::Completed,
        Ok(crate::torrent::TorrentWorkerExit::Paused) => DownloadStatus::Paused,
        Ok(crate::torrent::TorrentWorkerExit::Cancelled) => DownloadStatus::Cancelled,
        Err(ShioError::PasswordRequired) => DownloadStatus::PasswordRequired,
        Err(ShioError::Extract(_)) => DownloadStatus::ExtractError,
        Err(_) => DownloadStatus::Error,
    }
}

const fn effective_http_final_status(
    current: DownloadStatus,
    final_status: DownloadStatus,
) -> DownloadStatus {
    if matches!(final_status, DownloadStatus::Error)
        || matches!(
            final_status,
            DownloadStatus::Completed
                | DownloadStatus::ExtractError
                | DownloadStatus::PasswordRequired
        )
        || !matches!(current, DownloadStatus::Queued | DownloadStatus::Starting)
    {
        return final_status;
    }

    current
}

fn http_preview_error_state(error: ShioError) -> HttpPreviewState {
    match error {
        ShioError::NotADirectFile { .. } => HttpPreviewState::Blocked {
            reason: "not a direct file link".to_string(),
        },
        other => HttpPreviewState::Error {
            message: other.short_label(),
        },
    }
}

fn fragment_filename(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let fragment = parsed.fragment()?.trim();
    if fragment.is_empty() {
        return None;
    }
    Some(crate::filename::sanitize_filename(fragment))
}

fn http_preview_error_result(request_id: u64, error: ShioError) -> HttpPreviewResult {
    HttpPreviewResult {
        request_id,
        state: http_preview_error_state(error),
    }
}

async fn finalize_torrent(
    id: DownloadId,
    state: &Arc<EngineState>,
    db_writer: mpsc::Sender<DbWrite>,
    final_status: DownloadStatus,
    err_label: Option<String>,
) {
    let (effective_status, downloaded, total_size, torrent_runtime) = {
        let mut downloads = state.downloads.write();
        if let Some(d) = downloads.get_mut(&id) {
            let effective_status = match (d.status, final_status) {
                (DownloadStatus::Paused, _) => DownloadStatus::Paused,
                (DownloadStatus::Cancelled, _) => DownloadStatus::Cancelled,
                (DownloadStatus::Completed, _) => DownloadStatus::Completed,
                _ => final_status,
            };
            d.status = effective_status;
            d.speed = 0;
            d.avg_speed = 0;
            if matches!(effective_status, DownloadStatus::Completed) {
                d.completed_at = Some(chrono::Utc::now());
            }
            d.error_message = matches!(
                effective_status,
                DownloadStatus::Error
                    | DownloadStatus::ExtractError
                    | DownloadStatus::PasswordRequired
            )
            .then_some(err_label)
            .flatten();
            let torrent_runtime = d.torrent().map(|torrent| TorrentRuntimeWrite {
                uploaded: torrent.uploaded,
                ratio: torrent.ratio,
                seed_elapsed_secs: torrent.seed_elapsed_secs,
            });
            (
                effective_status,
                d.downloaded,
                d.total_size,
                torrent_runtime,
            )
        } else {
            (final_status, 0, None, None)
        }
    };

    if let Err(e) = db_writer
        .send(DbWrite::Final {
            id,
            status: effective_status,
            downloaded,
            total_size,
            torrent_runtime,
        })
        .await
    {
        tracing::warn!(download = %id, "final torrent db write dropped: {e}");
    }
}

fn restore_downloads(state: &EngineState, restored: Vec<Download>) -> (usize, usize) {
    let mut downloads = state.downloads.write();
    let mut queue = state.queue.lock();
    let mut requeued = 0;
    let total = restored.len();
    for mut d in restored {
        let resumable = matches!(
            d.status,
            DownloadStatus::Downloading
                | DownloadStatus::Starting
                | DownloadStatus::Queued
                | DownloadStatus::Pending
                | DownloadStatus::Seeding
        );
        if resumable {
            d.status = DownloadStatus::Queued;
            d.speed = 0;
            d.avg_speed = 0;
            queue.push(d.id, d.priority);
            requeued += 1;
        } else if d.status == DownloadStatus::Extracting {
            d.status = DownloadStatus::ExtractError;
            d.error_message = Some("extract interrupted".to_string());
        }
        downloads.insert(d.id, d);
    }
    (total, requeued)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TorrentFile;
    use tokio::sync::oneshot;

    #[test]
    fn http_url_validation_accepts_only_http_urls_with_hosts() {
        for url in [
            "http://example.com/file.bin",
            "https://example.com/file.bin",
        ] {
            assert!(validate_http_url(url).is_ok());
        }

        for url in [
            "",
            "file:///tmp/file.bin",
            "ftp://example.com/file.bin",
            "magnet:?xt=urn:btih:abc",
            "/relative.bin",
            "http://",
        ] {
            assert!(validate_http_url(url).is_err());
        }
    }

    #[tokio::test]
    async fn add_rejects_invalid_http_url_before_state_or_db() {
        let db = Database::in_memory().unwrap();
        let (engine, _rx) = DownloadEngine::with_database(AppConfig::default(), db).unwrap();
        let download = Download::new(
            "ftp://example.com/file.bin".to_string(),
            std::path::PathBuf::from("/tmp"),
        );
        let id = download.id;

        assert!(engine.add(download).is_err());

        assert!(engine.downloads().is_empty());
        assert!(engine.db.get(id).unwrap().is_none());
    }

    #[tokio::test]
    async fn invalid_add_command_returns_error_acknowledgement() {
        let db = Database::in_memory().unwrap();
        let (engine, _rx) = DownloadEngine::with_database(AppConfig::default(), db).unwrap();
        let request = HttpDownloadRequest::new(
            "ftp://example.com/file.bin".to_string(),
            std::path::PathBuf::from("/tmp"),
        );
        let id = request.id();
        let (reply, ack) = oneshot::channel();

        engine
            .handle_command(EngineCommand::AddHttp { request, reply })
            .await;

        assert!(ack.await.unwrap().is_err());
        assert!(engine.downloads().is_empty());
        assert!(engine.db.get(id).unwrap().is_none());
    }

    #[test]
    fn archive_set_request_rejects_missing_parts_and_duplicate_network_urls() {
        let part1 = HttpArchivePartRequest::new(
            "https://example.com/a#Game.part01.rar".to_string(),
            "Game.part01.rar".to_string(),
            1,
            8,
        )
        .unwrap();
        let part3 = HttpArchivePartRequest::new(
            "https://example.com/c#Game.part03.rar".to_string(),
            "Game.part03.rar".to_string(),
            3,
            8,
        )
        .unwrap();

        assert!(
            HttpArchiveSetRequest::new(
                "Game".to_string(),
                std::path::PathBuf::from("downloads"),
                vec![part1.clone(), part3],
                true,
            )
            .is_err()
        );

        let duplicate = HttpArchivePartRequest::new(
            "https://example.com/a#Game.part02.rar".to_string(),
            "Game.part02.rar".to_string(),
            2,
            8,
        )
        .unwrap();
        assert!(
            HttpArchiveSetRequest::new(
                "Game".to_string(),
                std::path::PathBuf::from("downloads"),
                vec![part1, duplicate],
                true,
            )
            .is_err()
        );
    }

    #[test]
    fn prepared_torrent_request_preserves_id() {
        let id = DownloadId::new();
        let request = TorrentDownloadRequest::new(
            crate::types::TorrentSource::Magnet(
                "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862".into(),
            ),
            std::path::PathBuf::from("downloads"),
        )
        .with_id(id);

        let download = request.into_download().unwrap();

        assert_eq!(download.id, id);
    }

    #[tokio::test]
    async fn add_does_not_publish_state_when_insert_fails() {
        let db = Database::in_memory().unwrap();
        let (engine, _rx) = DownloadEngine::with_database(AppConfig::default(), db).unwrap();
        let mut first = Download::new(
            "https://example.com/first.bin".to_string(),
            std::path::PathBuf::from("/tmp"),
        );
        first.filename = "first.bin".to_string();
        let mut duplicate = Download::new(
            "https://example.com/duplicate.bin".to_string(),
            std::path::PathBuf::from("/tmp"),
        );
        duplicate.id = first.id;
        duplicate.filename = "duplicate.bin".to_string();
        let id = first.id;

        engine.add(first).unwrap();
        assert!(engine.add(duplicate).is_err());

        let downloads = engine.downloads();
        assert_eq!(
            downloads
                .iter()
                .find(|download| download.id == id)
                .map(|download| download.filename.as_str()),
            Some("first.bin")
        );
    }

    fn torrent_download(save_path: std::path::PathBuf) -> Download {
        Download::try_from_torrent(
            crate::types::TorrentSource::Magnet(
                "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862".into(),
            ),
            save_path,
        )
        .expect("valid magnet")
    }

    #[tokio::test]
    async fn remove_torrent_with_delete_files_deletes_selected_payload_without_session() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Database::in_memory().unwrap();
        let (engine, _rx) = DownloadEngine::with_database(AppConfig::default(), db).unwrap();
        let mut download = torrent_download(tmp.path().to_path_buf());
        let selected = tmp.path().join("release/selected.bin");
        let deselected = tmp.path().join("release/deselected.bin");
        tokio::fs::create_dir_all(selected.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&selected, b"selected").await.unwrap();
        tokio::fs::write(&deselected, b"deselected").await.unwrap();
        download.torrent_mut().unwrap().files = vec![
            TorrentFile {
                path: std::path::PathBuf::from("release/selected.bin"),
                size: 8,
                downloaded: 8,
                selected: true,
            },
            TorrentFile {
                path: std::path::PathBuf::from("release/deselected.bin"),
                size: 10,
                downloaded: 10,
                selected: false,
            },
        ];
        let id = download.id;
        engine.add(download).unwrap();

        engine.remove(id, true).await.unwrap();

        assert!(!selected.exists());
        assert!(deselected.exists());
        assert!(engine.db.get(id).unwrap().is_none());
    }

    #[tokio::test]
    async fn failed_delete_files_remove_ack_keeps_download_state() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Database::in_memory().unwrap();
        let (engine, _rx) = DownloadEngine::with_database(AppConfig::default(), db).unwrap();
        let mut download = Download::new(
            "https://example.com/payload.bin".to_string(),
            tmp.path().to_path_buf(),
        );
        download.filename = "payload.bin".to_string();
        let id = download.id;
        tokio::fs::create_dir(tmp.path().join("payload.bin"))
            .await
            .unwrap();
        engine.add(download).unwrap();
        let (reply, ack) = oneshot::channel();

        engine
            .handle_command(EngineCommand::Remove {
                id,
                delete_files: true,
                reply,
            })
            .await;

        assert!(ack.await.unwrap().is_err());
        assert!(engine.downloads().iter().any(|download| download.id == id));
        assert!(engine.db.get(id).unwrap().is_some());
    }

    #[tokio::test]
    async fn cancel_during_extract_returns_error_and_keeps_extracting_state() {
        let db = Database::in_memory().unwrap();
        let (engine, _rx) = DownloadEngine::with_database(AppConfig::default(), db).unwrap();
        let mut download = Download::new(
            "https://example.com/archive.zip".to_string(),
            std::path::PathBuf::from("/tmp"),
        );
        download.filename = "archive.zip".to_string();
        let id = download.id;
        engine.add(download).unwrap();
        {
            let mut downloads = engine.state.downloads.write();
            downloads.get_mut(&id).unwrap().status = DownloadStatus::Extracting;
        }
        engine
            .db
            .update_status(id, DownloadStatus::Extracting, None)
            .unwrap();
        let (reply, ack) = oneshot::channel();

        engine
            .handle_command(EngineCommand::Cancel { id, reply })
            .await;

        assert!(ack.await.unwrap().is_err());
        assert_eq!(
            engine
                .downloads()
                .into_iter()
                .find(|download| download.id == id)
                .map(|download| download.status),
            Some(DownloadStatus::Extracting)
        );
        assert_eq!(
            engine.db.get(id).unwrap().map(|download| download.status),
            Some(DownloadStatus::Extracting)
        );
    }

    #[tokio::test]
    async fn remove_during_extract_returns_error_and_keeps_download_state() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Database::in_memory().unwrap();
        let (engine, _rx) = DownloadEngine::with_database(AppConfig::default(), db).unwrap();
        let mut download = Download::new(
            "https://example.com/archive.zip".to_string(),
            tmp.path().to_path_buf(),
        );
        download.filename = "archive.zip".to_string();
        let id = download.id;
        engine.add(download).unwrap();
        {
            let mut downloads = engine.state.downloads.write();
            downloads.get_mut(&id).unwrap().status = DownloadStatus::Extracting;
        }
        engine
            .db
            .update_status(id, DownloadStatus::Extracting, None)
            .unwrap();
        let (reply, ack) = oneshot::channel();

        engine
            .handle_command(EngineCommand::Remove {
                id,
                delete_files: true,
                reply,
            })
            .await;

        assert!(ack.await.unwrap().is_err());
        assert!(engine.downloads().iter().any(|download| download.id == id));
        assert_eq!(
            engine.db.get(id).unwrap().map(|download| download.status),
            Some(DownloadStatus::Extracting)
        );
    }

    #[tokio::test]
    async fn finalize_torrent_persists_runtime_snapshot() {
        let db = Database::in_memory().unwrap();
        let (engine, _rx) = DownloadEngine::with_database(AppConfig::default(), db).unwrap();
        let mut download = torrent_download(std::path::PathBuf::from("/tmp"));
        download.downloaded = 100;
        download.total_size = Some(100);
        let id = download.id;
        {
            let torrent = download.torrent_mut().unwrap();
            torrent.uploaded = 200;
            torrent.ratio = 2.0;
            torrent.seed_elapsed_secs = 30;
        }
        engine.add(download).unwrap();
        let db_writer = engine.db_writer_sender().unwrap();

        finalize_torrent(
            id,
            &engine.state,
            db_writer,
            DownloadStatus::Completed,
            None,
        )
        .await;

        for _ in 0..20 {
            let stored = engine.db.get(id).unwrap().unwrap();
            if stored.torrent().is_some_and(|torrent| {
                torrent.uploaded == 200
                    && (torrent.ratio - 2.0).abs() < f32::EPSILON
                    && torrent.seed_elapsed_secs == 30
            }) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("torrent runtime was not persisted");
    }

    #[test]
    fn non_blocked_http_preview_error_preserves_request_context() {
        let result = http_preview_error_result(10, ShioError::InvalidUrl("not a url".to_string()));

        assert_eq!(
            result,
            crate::types::HttpPreviewResult {
                request_id: 10,
                state: crate::types::HttpPreviewState::Error {
                    message: "invalid url".to_string(),
                },
            }
        );
    }

    #[test]
    fn torrent_worker_exit_maps_to_status() {
        use crate::torrent::TorrentWorkerExit;

        assert_eq!(
            torrent_worker_exit_status(&Ok(TorrentWorkerExit::Completed)),
            DownloadStatus::Completed
        );
        assert_eq!(
            torrent_worker_exit_status(&Ok(TorrentWorkerExit::SeedingStopped)),
            DownloadStatus::Completed
        );
        assert_eq!(
            torrent_worker_exit_status(&Ok(TorrentWorkerExit::Paused)),
            DownloadStatus::Paused
        );
        assert_eq!(
            torrent_worker_exit_status(&Ok(TorrentWorkerExit::Cancelled)),
            DownloadStatus::Cancelled
        );
        assert_eq!(
            torrent_worker_exit_status(&Err(ShioError::PasswordRequired)),
            DownloadStatus::PasswordRequired
        );
        assert_eq!(
            torrent_worker_exit_status(&Err(ShioError::Extract("crc".into()))),
            DownloadStatus::ExtractError
        );
    }

    #[test]
    fn restore_requeues_seeding_torrents() {
        let state = EngineState {
            downloads: RwLock::new(HashMap::new()),
            packages: RwLock::new(HashMap::new()),
            queue: Mutex::new(DownloadQueue::new()),
            active_count: AtomicU8::new(0),
            config: RwLock::new(AppConfig::default()),
            shutdown_token: CancellationToken::new(),
            active_cancels: Mutex::new(HashMap::new()),
            active_http_previews: Mutex::new(HashMap::new()),
            cancelled_http_previews: Mutex::new(HashSet::new()),
            active_workers: Mutex::new(Vec::new()),
            slot_notify: Notify::new(),
            torrent_session: OnceCell::new(),
        };
        let mut download = Download::try_from_torrent(
            crate::types::TorrentSource::Magnet(
                "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862".into(),
            ),
            std::path::PathBuf::from("/tmp"),
        )
        .expect("valid magnet");
        let id = download.id;
        download.status = DownloadStatus::Seeding;

        let (_, requeued) = restore_downloads(&state, vec![download]);

        assert_eq!(requeued, 1);
        assert_eq!(
            state.downloads.read().get(&id).map(|d| d.status),
            Some(DownloadStatus::Queued)
        );
    }

    #[test]
    fn restore_does_not_requeue_completed_torrents() {
        let state = EngineState {
            downloads: RwLock::new(HashMap::new()),
            packages: RwLock::new(HashMap::new()),
            queue: Mutex::new(DownloadQueue::new()),
            active_count: AtomicU8::new(0),
            config: RwLock::new(AppConfig::default()),
            shutdown_token: CancellationToken::new(),
            active_cancels: Mutex::new(HashMap::new()),
            active_http_previews: Mutex::new(HashMap::new()),
            cancelled_http_previews: Mutex::new(HashSet::new()),
            active_workers: Mutex::new(Vec::new()),
            slot_notify: Notify::new(),
            torrent_session: OnceCell::new(),
        };
        let mut download = Download::try_from_torrent(
            crate::types::TorrentSource::Magnet(
                "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862".into(),
            ),
            std::path::PathBuf::from("/tmp"),
        )
        .expect("valid magnet");
        let id = download.id;
        download.status = DownloadStatus::Completed;

        let (_, requeued) = restore_downloads(&state, vec![download]);

        assert_eq!(requeued, 0);
        assert_eq!(
            state.downloads.read().get(&id).map(|d| d.status),
            Some(DownloadStatus::Completed)
        );
    }

    #[test]
    fn restore_preserves_torrent_seed_elapsed() {
        let state = EngineState {
            downloads: RwLock::new(HashMap::new()),
            packages: RwLock::new(HashMap::new()),
            queue: Mutex::new(DownloadQueue::new()),
            active_count: AtomicU8::new(0),
            config: RwLock::new(AppConfig::default()),
            shutdown_token: CancellationToken::new(),
            active_cancels: Mutex::new(HashMap::new()),
            active_http_previews: Mutex::new(HashMap::new()),
            cancelled_http_previews: Mutex::new(HashSet::new()),
            active_workers: Mutex::new(Vec::new()),
            slot_notify: Notify::new(),
            torrent_session: OnceCell::new(),
        };
        let mut download = Download::try_from_torrent(
            crate::types::TorrentSource::Magnet(
                "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862".into(),
            ),
            std::path::PathBuf::from("/tmp"),
        )
        .expect("valid magnet");
        let id = download.id;
        download.status = DownloadStatus::Seeding;
        download.torrent_mut().expect("torrent").seed_elapsed_secs = 3600;

        let (_, requeued) = restore_downloads(&state, vec![download]);

        assert_eq!(requeued, 1);
        assert_eq!(
            state
                .downloads
                .read()
                .get(&id)
                .and_then(|d| d.torrent())
                .map(|torrent| torrent.seed_elapsed_secs),
            Some(3600)
        );
    }

    #[test]
    fn new_fails_when_persisted_downloads_are_corrupt() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let db = Database::open(tmp.path()).unwrap();
        let download = Download::new(
            "https://example.com/file.bin".to_string(),
            std::path::PathBuf::from("/tmp/file.bin"),
        );
        db.insert_download(&download).unwrap();
        drop(db);
        let conn = rusqlite::Connection::open(tmp.path()).unwrap();
        conn.execute(
            "UPDATE http_state SET headers = ?2 WHERE download_id = ?1",
            rusqlite::params![download.id.0.to_string(), "not json"],
        )
        .unwrap();
        drop(conn);

        let result = DownloadEngine::new(AppConfig::default(), tmp.path());

        assert!(result.is_err());
    }
}
