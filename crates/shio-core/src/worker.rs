use crate::chunk::{BandwidthLimiter, Exit, Pieces, Transfer, connect, plan_pieces};
use crate::config::AppConfig;
use crate::db::Database;
use crate::error::{Result, ShioError};
use crate::extract::{Plan, Readiness};
use crate::probe::HttpProbe;
use crate::types::{
    ChunkInfo, Download, DownloadId, DownloadProgress, DownloadStatus, HttpState, ProxyProtocol,
};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Semaphore, mpsc, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

pub(crate) const USER_AGENT: &str = concat!("Shio/", env!("CARGO_PKG_VERSION"));
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(10);
const SPEED_WINDOW: Duration = Duration::from_secs(3);
const MAX_RESUME_METADATA_BYTES: u64 = 16 * 1024;

pub(crate) fn build_client(config: &AppConfig) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .redirect(reqwest::redirect::Policy::limited(10))
        .referer(false)
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(30))
        .pool_idle_timeout(Duration::from_secs(90))
        .default_headers(reqwest::header::HeaderMap::from_iter([(
            reqwest::header::ACCEPT_ENCODING,
            reqwest::header::HeaderValue::from_static("identity"),
        )]))
        .cookie_store(true);
    if let Some(proxy) = &config.proxy {
        let scheme = match proxy.protocol {
            ProxyProtocol::Http => "http://proxy",
            ProxyProtocol::Socks5 => "socks5h://proxy",
        };
        let mut url = url::Url::parse(scheme).expect("proxy URL template is valid");
        let host = match proxy.host.parse::<std::net::IpAddr>() {
            Ok(ip) => url.set_ip_host(ip),
            Err(_) => url.set_host(Some(&proxy.host)).map_err(drop),
        };
        host.and_then(|()| url.set_port(Some(proxy.port)))
            .map_err(|()| ShioError::Config("invalid proxy host".into()))?;
        let mut proxy_config = reqwest::Proxy::all(url)?;
        if let (Some(user), Some(password)) = (&proxy.username, &proxy.password) {
            proxy_config = proxy_config.basic_auth(user, password);
        }
        builder = builder.proxy(proxy_config);
    }
    Ok(builder.build()?)
}

pub(crate) fn url_without_fragment(url: &str) -> String {
    url::Url::parse(url).map_or_else(
        |_| url.to_owned(),
        |mut parsed| {
            parsed.set_fragment(None);
            parsed.into()
        },
    )
}

pub(crate) struct HttpJob {
    pub(crate) config: watch::Receiver<AppConfig>,
    pub(crate) client: reqwest::Client,
    pub(crate) db: Arc<Database>,
    pub(crate) progress_tx: mpsc::Sender<DownloadProgress>,
    pub(crate) cancel: CancellationToken,
    pub(crate) limiter: BandwidthLimiter,
    pub(crate) host: Arc<Semaphore>,
}

pub(crate) async fn run(download: Download, probe: HttpProbe, job: HttpJob) -> Result<()> {
    let http = download
        .http()
        .ok_or_else(|| ShioError::Other("expected an HTTP download".into()))?;
    if probe
        .content_length
        .is_some_and(|length| i64::try_from(length).is_err())
    {
        return Err(ShioError::Other("remote file is too large".into()));
    }
    let published = {
        let download = download.clone();
        blocking(move || prepare_destination(&download)).await?
    };
    let size = match published {
        Some(size) => size,
        None => download_payload(&download, http, &probe, &job).await?,
    };
    if http.auto_extract {
        auto_extract(&download, size, &job).await
    } else {
        Ok(())
    }
}

/// `Some(size)` when an earlier run already published the payload.
fn prepare_destination(download: &Download) -> Result<Option<u64>> {
    ensure_output_directory(download)?;
    ensure_owned_staging(download)?;
    let destination = download.file_path();
    let published = recover_publication(&Staging::of(download), &destination)?;
    if published.is_none() && std::fs::symlink_metadata(&destination).is_ok() {
        return Err(ShioError::DestinationExists(destination));
    }
    Ok(published)
}

async fn download_payload(
    download: &Download,
    http: &HttpState,
    probe: &HttpProbe,
    job: &HttpJob,
) -> Result<u64> {
    let total = probe.content_length;
    let staging = Staging::of(download);
    let identity = ResumeMetadata {
        content_length: total,
        validator: probe.validator.clone(),
        ..ResumeMetadata::default()
    };
    let ranged = probe.accept_ranges && probe.validator.is_some();
    let planned = plan_pieces(total, ranged);
    let (chunks, file) = {
        let (db, id, staging, identity) = (
            job.db.clone(),
            download.id,
            staging.clone(),
            identity.clone(),
        );
        blocking(move || prepare_payload(&db, id, &staging, &identity, planned, total, ranged))
            .await?
    };
    let if_range = probe
        .validator
        .as_deref()
        .map(reqwest::header::HeaderValue::from_str)
        .transpose()
        .map_err(|_| ShioError::Other("invalid resume validator".into()))?;
    let (ranged_tx, mut ranged_rx) = mpsc::unbounded_channel();
    let transfer = Arc::new(Transfer {
        client: job.client.clone(),
        // Connections request the original URL so reqwest strips credentials on every
        // cross-origin redirect and short-lived signed redirect targets are re-resolved.
        url: url_without_fragment(&http.url),
        headers: http.headers.clone(),
        file: Arc::new(file),
        if_range,
        total,
        require_range: chunks.len() > 1,
        pieces: Pieces::new(chunks, total),
        limiter: job.limiter.clone(),
        cancel: job.cancel.child_token(),
        max_retries: download.max_retries,
        ranged: ranged_tx,
    });

    let mut target = usize::from(http.segments);
    let mut connections = JoinSet::new();
    let mut failure = None;
    let mut samples = VecDeque::from([(Instant::now(), transfer.pieces.downloaded())]);
    let mut saving: Option<JoinHandle<()>> = None;
    let mut saved_at = Instant::now();
    let mut tick = tokio::time::interval(PROGRESS_INTERVAL);
    loop {
        if connections.is_empty() {
            if failure.is_some() || transfer.cancel.is_cancelled() || transfer.pieces.is_complete()
            {
                break;
            }
            let permit = tokio::select! {
                () = transfer.cancel.cancelled() => break,
                permit = job.host.clone().acquire_owned() => {
                    permit.expect("host semaphores are never closed")
                },
            };
            let Some(piece) = transfer.pieces.claim() else {
                break;
            };
            connections.spawn(connect(transfer.clone(), piece, permit));
        }
        tokio::select! {
            Some(joined) = connections.join_next() => {
                let result = joined.unwrap_or_else(|error| {
                    Err(ShioError::Other(format!("connection task failed: {error}")))
                });
                match result {
                    Ok(Exit::Done) => {},
                    Ok(Exit::Refused) => target = target.saturating_sub(1).max(1),
                    Err(error) => {
                        if !matches!(error, ShioError::Cancelled) {
                            transfer.cancel.cancel();
                        }
                        if failure
                            .as_ref()
                            .is_none_or(|current| matches!(current, ShioError::Cancelled))
                        {
                            failure = Some(error);
                        }
                    },
                }
            },
            Some(()) = ranged_rx.recv() => {
                if connections.len() < target
                    && let Ok(permit) = job.host.clone().try_acquire_owned()
                    && let Some(piece) = transfer.pieces.claim()
                {
                    connections.spawn(connect(transfer.clone(), piece, permit));
                }
            },
            now = tick.tick() => {
                let now = now.into_std();
                let bytes = transfer.pieces.downloaded();
                samples.push_back((now, bytes));
                while samples
                    .front()
                    .is_some_and(|(at, _)| now.saturating_duration_since(*at) > SPEED_WINDOW)
                {
                    samples.pop_front();
                }
                let speed = samples.front().map_or(0, |(at, then)| {
                    let elapsed = now.saturating_duration_since(*at).as_millis().max(1);
                    let bytes = u128::from(bytes.saturating_sub(*then));
                    u64::try_from(bytes * 1000 / elapsed).unwrap_or(u64::MAX)
                });
                let _ = job.progress_tx.try_send(DownloadProgress {
                    id: download.id,
                    status: DownloadStatus::Downloading,
                    downloaded: bytes,
                    total_size: total,
                    speed,
                    torrent: None,
                });
                if saved_at.elapsed() >= CHECKPOINT_INTERVAL
                    && saving.as_ref().is_none_or(JoinHandle::is_finished)
                {
                    saving = Some(tokio::spawn(checkpoint(
                        download.id,
                        transfer.clone(),
                        job.db.clone(),
                    )));
                    saved_at = Instant::now();
                }
            },
        }
    }
    if let Some(saving) = saving {
        let _ = saving.await;
    }
    checkpoint(download.id, transfer.clone(), job.db.clone()).await;
    if let Some(error) = failure {
        return Err(error);
    }
    if job.cancel.is_cancelled() {
        return Err(ShioError::Cancelled);
    }

    let size = transfer.pieces.downloaded();
    if let Some(expected) = total
        && size != expected
    {
        return Err(ShioError::SizeMismatch {
            expected,
            actual: size,
        });
    }
    let destination = download.file_path();
    blocking(move || publish(&staging, &destination, identity, &transfer.file, size)).await?;
    Ok(size)
}

/// Resumes only when the saved offsets, the resource identity and the file on disk agree;
/// otherwise the offsets are reset before the payload is truncated.
fn prepare_payload(
    db: &Database,
    id: DownloadId,
    staging: &Staging,
    identity: &ResumeMetadata,
    planned: Vec<ChunkInfo>,
    total: Option<u64>,
    ranged: bool,
) -> Result<(Vec<ChunkInfo>, File)> {
    let saved = db.chunks(id)?;
    let existing = std::fs::OpenOptions::new()
        .write(true)
        .open(&staging.partial)
        .ok();
    let resumable = existing.as_ref().is_some_and(|file| {
        let len = file.metadata().map_or(0, |metadata| metadata.len());
        resumable(&saved, total, ranged, len)
            && read_resume_metadata(&staging.metadata)
                .is_some_and(|saved| saved.same_resource(identity))
    });
    let (chunks, file) = match existing {
        Some(file) if resumable => {
            if total.is_none() {
                file.set_len(saved[0].downloaded)?;
            }
            (saved, file)
        },
        _ => {
            db.replace_chunks(id, &planned)?;
            (planned, File::create(&staging.partial)?)
        },
    };
    #[cfg(windows)]
    if let Err(error) = mark_sparse(&file) {
        tracing::warn!(download = %id, "could not make the partial file sparse: {error}");
    }
    write_resume_metadata(&staging.metadata, identity)?;
    Ok((chunks, file))
}

/// The saved layout must tile `[0, total)`, and counted bytes must lie within the file on disk.
fn resumable(saved: &[ChunkInfo], total: Option<u64>, ranged: bool, file_len: u64) -> bool {
    let Some(total) = total else {
        let [only] = saved else { return false };
        return only.index == 0 && (1..=file_len).contains(&only.downloaded);
    };
    let mut next = 0_u64;
    let tiles = saved.iter().zip(0_u32..).all(|(chunk, index)| {
        let fits = chunk.index == index
            && chunk.start == next
            && chunk.end >= chunk.start
            && chunk.downloaded <= chunk.end - chunk.start + 1
            && (chunk.downloaded == 0 || chunk.start.saturating_add(chunk.downloaded) <= file_len);
        next = chunk.end.saturating_add(1);
        fits
    });
    tiles
        && next == total
        && file_len <= total
        && (ranged || saved.len() == 1)
        && saved.iter().any(|chunk| chunk.downloaded > 0)
}

/// Writing far into a file makes NTFS zero-fill everything before it unless it is sparse.
#[cfg(windows)]
#[allow(unsafe_code)]
fn mark_sparse(file: &File) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::FSCTL_SET_SPARSE;

    let mut returned = 0_u32;
    // SAFETY: the handle stays open for the call; FSCTL_SET_SPARSE takes no buffers; a
    // synchronous call needs a valid `lpBytesReturned`, which `returned` is.
    let ok = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            FSCTL_SET_SPARSE,
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            0,
            &raw mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Offsets are persisted only after the bytes they count are synced, so a crash never resumes
/// past lost data.
async fn checkpoint(id: DownloadId, transfer: Arc<Transfer>, db: Arc<Database>) {
    let rows = transfer.pieces.unsaved();
    if rows.is_empty() {
        return;
    }
    let file = transfer.file.clone();
    let result = blocking(move || {
        file.sync_data()?;
        db.update_chunks(id, &rows)?;
        Ok(rows)
    })
    .await;
    match result {
        Ok(rows) => transfer.pieces.saved(&rows),
        Err(error) => {
            tracing::warn!(download = %id, "chunk checkpoint failed: {}", error.safe_diagnostic());
        },
    }
}

async fn auto_extract(download: &Download, size: u64, job: &HttpJob) -> Result<()> {
    match extraction_readiness(download).await {
        None => Ok(()),
        Some(Readiness::PasswordRequired) => Err(ShioError::PasswordRequired),
        Some(Readiness::Incomplete) => Err(ShioError::Extract("archive set is incomplete".into())),
        Some(Readiness::Ready) => {
            let _ = job
                .progress_tx
                .send(DownloadProgress {
                    id: download.id,
                    status: DownloadStatus::Extracting,
                    downloaded: size,
                    total_size: Some(size),
                    speed: 0,
                    torrent: None,
                })
                .await;
            let config = job.config.borrow().clone();
            run_extract(
                std::slice::from_ref(download),
                &config,
                None,
                job.cancel.clone(),
            )
            .await
        },
    }
}

pub(crate) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| ShioError::Other(format!("blocking task failed: {error}")))?
}

pub(crate) fn http_staging_root(download: &Download) -> PathBuf {
    download
        .file_dir()
        .join(format!(".shio-{}.http", download.id))
}

#[derive(Clone)]
struct Staging {
    root: PathBuf,
    partial: PathBuf,
    metadata: PathBuf,
}

impl Staging {
    fn of(download: &Download) -> Self {
        let root = http_staging_root(download);
        Self {
            partial: root.join("payload.part"),
            metadata: root.join("resume.json"),
            root,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct ResumeMetadata {
    content_length: Option<u64>,
    validator: Option<String>,
    publish_intent_size: Option<u64>,
    published_size: Option<u64>,
}

impl ResumeMetadata {
    fn same_resource(&self, other: &Self) -> bool {
        self.validator.is_some()
            && self.validator == other.validator
            && self.content_length == other.content_length
    }
}

fn ensure_output_directory(download: &Download) -> Result<()> {
    std::fs::create_dir_all(&download.save_path)?;
    let directory = download.file_dir();
    if directory == download.save_path {
        return Ok(());
    }
    match std::fs::create_dir(&directory) {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if is_real_dir(&directory) {
                Ok(())
            } else {
                Err(ShioError::DestinationExists(directory))
            }
        },
        result => Ok(result?),
    }
}

/// A root left without its owner file by a crash during creation is adopted only while it
/// holds no payload.
fn ensure_owned_staging(download: &Download) -> Result<()> {
    let staging = Staging::of(download);
    let owner = staging.root.join("owner");
    let id = download.id.to_string();
    match std::fs::create_dir(&staging.root) {
        Ok(()) => {},
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if !is_real_dir(&staging.root) {
                return Err(ShioError::DestinationExists(staging.root));
            }
            let recorded = match std::fs::symlink_metadata(&owner) {
                Ok(metadata) if metadata.is_file() && metadata.len() <= 64 => {
                    std::fs::read(&owner)?
                },
                Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
                _ => return Err(ShioError::DestinationExists(staging.root)),
            };
            if recorded == id.as_bytes() {
                return Ok(());
            }
            let holds_payload = [&staging.partial, &staging.metadata]
                .iter()
                .any(|path| std::fs::symlink_metadata(path).is_ok());
            if !recorded.is_empty() || holds_payload {
                return Err(ShioError::DestinationExists(staging.root));
            }
        },
        Err(error) => return Err(error.into()),
    }
    #[cfg(unix)]
    std::fs::set_permissions(
        &staging.root,
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )?;
    let mut file = File::create(&owner)?;
    file.write_all(id.as_bytes())?;
    file.sync_all()?;
    sync_parent(&owner)
}

/// Finishes a publish interrupted by a crash; `Some(size)` means the destination is Shio's
/// complete payload.
fn recover_publication(staging: &Staging, destination: &Path) -> Result<Option<u64>> {
    let Some(mut metadata) = read_resume_metadata(&staging.metadata) else {
        return Ok(None);
    };
    let Some(size) = metadata.published_size.or(metadata.publish_intent_size) else {
        return Ok(None);
    };
    let recorded = metadata.published_size.is_some();
    if regular_file_len(destination) != Some(size)
        || (!recorded
            && regular_file_len(&staging.partial).is_some()
            && !files_equal(&staging.partial, destination)?)
    {
        return Ok(None);
    }
    remove_if_exists(&staging.partial)?;
    if !recorded {
        metadata.publish_intent_size = None;
        metadata.published_size = Some(size);
        write_resume_metadata(&staging.metadata, &metadata)?;
    }
    Ok(Some(size))
}

fn publish(
    staging: &Staging,
    destination: &Path,
    mut metadata: ResumeMetadata,
    file: &File,
    size: u64,
) -> Result<()> {
    file.sync_all()?;
    metadata.publish_intent_size = Some(size);
    write_resume_metadata(&staging.metadata, &metadata)?;
    if let Err(error) = rename_no_clobber(&staging.partial, destination) {
        metadata.publish_intent_size = None;
        write_resume_metadata(&staging.metadata, &metadata)?;
        return Err(if error.kind() == io::ErrorKind::AlreadyExists {
            ShioError::DestinationExists(destination.to_owned())
        } else {
            error.into()
        });
    }
    sync_parent(destination)?;
    metadata.publish_intent_size = None;
    metadata.published_size = Some(size);
    write_resume_metadata(&staging.metadata, &metadata)
}

/// The completed file this download published, if it is still the one Shio wrote.
pub(crate) fn published_file(download: &Download) -> Option<PathBuf> {
    let size = match read_resume_metadata(&Staging::of(download).metadata) {
        Some(metadata) => metadata.published_size?,
        // `settle_publication` moved the size into the durable row and dropped the staging.
        None if download.status.is_downloaded() => download.total_size?,
        None => return None,
    };
    let path = download.file_path();
    (regular_file_len(&path) == Some(size)).then_some(path)
}

/// Moves the published size into the row; drop the staging only once that row is durable.
pub(crate) fn settle_publication(download: &mut Download) -> bool {
    let Some(size) =
        read_resume_metadata(&Staging::of(download).metadata).and_then(|m| m.published_size)
    else {
        return false;
    };
    download.total_size = Some(size);
    download.downloaded = size;
    true
}

fn read_resume_metadata(path: &Path) -> Option<ResumeMetadata> {
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(MAX_RESUME_METADATA_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_RESUME_METADATA_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

fn write_resume_metadata(path: &Path, metadata: &ResumeMetadata) -> Result<()> {
    let bytes = serde_json::to_vec(metadata)
        .map_err(|error| ShioError::Other(format!("encode resume metadata: {error}")))?;
    let parent = path
        .parent()
        .ok_or_else(|| ShioError::Other("resume metadata path has no parent".into()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| ShioError::Io(error.error))?;
    sync_parent(path)
}

/// Atomic rename that fails with `AlreadyExists` instead of replacing, for files and
/// directories; `from` is never deleted.
pub(crate) fn rename_no_clobber(from: &Path, to: &Path) -> io::Result<()> {
    let mut path = tempfile::TempPath::try_from_path(from)?;
    path.disable_cleanup(true);
    path.persist_noclobber(to).map_err(|error| error.error)
}

/// Publishes `source` as a hard link, never clobbering or exposing a partial file. Without
/// link support it is moved when `may_move`, else copied via a synced temporary; returns
/// whether it moved.
pub(crate) async fn publish_file_no_clobber(
    source: &Path,
    destination: &Path,
    may_move: bool,
) -> Result<bool> {
    let (source, destination) = (source.to_owned(), destination.to_owned());
    blocking(move || {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&source)?
            .sync_all()?;
        let exists = |error: &io::Error| error.kind() == io::ErrorKind::AlreadyExists;
        let moved = match std::fs::hard_link(&source, &destination) {
            Ok(()) => false,
            Err(error) if exists(&error) => return Err(ShioError::DestinationExists(destination)),
            Err(_) => {
                let moved = may_move
                    && match rename_no_clobber(&source, &destination) {
                        Ok(()) => true,
                        Err(error) if exists(&error) => {
                            return Err(ShioError::DestinationExists(destination));
                        },
                        Err(_) => false,
                    };
                if !moved {
                    let parent = destination
                        .parent()
                        .ok_or_else(|| ShioError::Other("published path has no parent".into()))?;
                    let mut temporary = tempfile::Builder::new()
                        .prefix(".shio-publish-")
                        .tempfile_in(parent)?;
                    io::copy(&mut File::open(&source)?, temporary.as_file_mut())?;
                    temporary.as_file().sync_all()?;
                    temporary.persist_noclobber(&destination).map_err(|error| {
                        if exists(&error.error) {
                            ShioError::DestinationExists(destination.clone())
                        } else {
                            error.error.into()
                        }
                    })?;
                }
                moved
            },
        };
        sync_parent(&destination)?;
        Ok(moved)
    })
    .await
}

pub(crate) async fn same_file(left: &Path, right: &Path) -> Result<bool> {
    let left = tokio::fs::symlink_metadata(left).await?;
    let right = tokio::fs::symlink_metadata(right).await?;
    if !left.is_file() || !right.is_file() {
        return Ok(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        Ok(left.dev() == right.dev() && left.ino() == right.ino())
    }
    #[cfg(not(unix))]
    Ok(false)
}

fn files_equal(left: &Path, right: &Path) -> Result<bool> {
    if regular_file_len(left) != regular_file_len(right) {
        return Ok(false);
    }
    let (mut left, mut right) = (File::open(left)?, File::open(right)?);
    let (mut left_buffer, mut right_buffer) = (vec![0_u8; 64 * 1024], vec![0_u8; 64 * 1024]);
    loop {
        let read = left.read(&mut left_buffer)?;
        if read == 0 {
            return Ok(true);
        }
        right.read_exact(&mut right_buffer[..read])?;
        if left_buffer[..read] != right_buffer[..read] {
            return Ok(false);
        }
    }
}

fn regular_file_len(path: &Path) -> Option<u64> {
    std::fs::symlink_metadata(path)
        .ok()
        .filter(std::fs::Metadata::is_file)
        .map(|metadata| metadata.len())
}

fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir())
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

pub(crate) fn sync_parent(path: &Path) -> Result<()> {
    if cfg!(unix)
        && let Some(parent) = path.parent()
    {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

pub(crate) async fn sync_parent_directory(path: &Path) -> Result<()> {
    let path = path.to_owned();
    blocking(move || sync_parent(&path)).await
}

fn archive_files(download: &Download) -> Vec<PathBuf> {
    let files: Vec<PathBuf> = match download.torrent() {
        Some(torrent) => torrent
            .files
            .iter()
            .filter(|file| file.selected)
            .map(|file| download.save_path.join(&file.path))
            .collect(),
        None => vec![download.file_path()],
    };
    files
        .into_iter()
        .filter(|file| {
            file.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| crate::extract::Format::of(name).is_some())
        })
        .collect()
}

/// The one archive set formed by `files`; several distinct sets are ambiguous.
fn archive_plan(files: &[PathBuf]) -> Option<Plan> {
    let mut plans: Vec<Plan> = Vec::new();
    for file in files {
        if !plans.iter().any(|plan| plan.members.contains(file)) {
            plans.extend(crate::extract::plan(file));
        }
    }
    if plans.len() == 1 { plans.pop() } else { None }
}

pub(crate) async fn extraction_readiness(download: &Download) -> Option<Readiness> {
    let files = archive_files(download);
    if files.is_empty() {
        return None;
    }
    let readiness = blocking(move || {
        Ok(archive_plan(&files).map_or(Readiness::Incomplete, |plan| plan.readiness()))
    });
    Some(readiness.await.unwrap_or(Readiness::Incomplete))
}

/// Extracts the set formed by `parts`, then deletes only volumes those parts wrote, never a
/// user's look-alike files.
pub(crate) async fn run_extract(
    parts: &[Download],
    config: &AppConfig,
    password: Option<String>,
    cancel: CancellationToken,
) -> Result<()> {
    let owned: Vec<PathBuf> = parts.iter().flat_map(archive_files).collect();
    let delete_archives = config.delete_archive_after_extract;
    blocking(move || {
        let plan = archive_plan(&owned)
            .ok_or_else(|| ShioError::Extract("no extractable archive found".into()))?;
        let destination = crate::extract::extract(&plan, password.as_deref(), &cancel)?;
        tracing::info!(
            "extracted {} -> {}",
            plan.first_volume.display(),
            destination.display()
        );
        if delete_archives {
            for member in plan.members.iter().filter(|member| owned.contains(member)) {
                if let Err(error) = std::fs::remove_file(member) {
                    tracing::warn!("failed to delete {}: {error}", member.display());
                }
            }
        }
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn download_in(dir: &Path, name: &str) -> Download {
        let mut download = Download::new(format!("https://example.com/{name}"), dir.to_owned());
        download.filename = name.into();
        download
    }

    #[test]
    fn proxy_hosts_are_built_as_urls() {
        for host in ["::1", "[::1]", "127.0.0.1", "proxy.example"] {
            let config = AppConfig {
                proxy: Some(crate::types::ProxyConfig {
                    protocol: ProxyProtocol::Socks5,
                    host: host.into(),
                    port: 1080,
                    username: None,
                    password: None,
                }),
                ..AppConfig::default()
            };
            assert!(build_client(&config).is_ok(), "{host}");
        }
    }

    #[test]
    fn publish_moves_the_payload_without_clobbering() {
        let temp = tempfile::tempdir().expect("tempdir");
        let download = download_in(temp.path(), "payload.bin");
        ensure_owned_staging(&download).expect("staging");
        let staging = Staging::of(&download);
        let destination = download.file_path();
        std::fs::write(&staging.partial, b"new").expect("partial");
        std::fs::write(&destination, b"existing").expect("destination");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&staging.partial)
            .expect("open");

        let result = publish(&staging, &destination, ResumeMetadata::default(), &file, 3);

        assert!(
            matches!(result, Err(ShioError::DestinationExists(_))),
            "{result:?}"
        );
        assert_eq!(
            std::fs::read(&destination).expect("destination"),
            b"existing"
        );
        assert_eq!(
            read_resume_metadata(&staging.metadata),
            Some(ResumeMetadata::default())
        );
        std::fs::remove_file(&destination).expect("remove");

        publish(&staging, &destination, ResumeMetadata::default(), &file, 3).expect("publish");

        assert_eq!(std::fs::read(&destination).expect("destination"), b"new");
        assert!(!staging.partial.exists());
        assert_eq!(published_file(&download), Some(destination));
    }

    #[test]
    fn interrupted_publication_is_recovered_only_for_shio_payloads() {
        let temp = tempfile::tempdir().expect("tempdir");
        let download = download_in(temp.path(), "payload.bin");
        ensure_owned_staging(&download).expect("staging");
        let staging = Staging::of(&download);
        let destination = download.file_path();
        let intent = ResumeMetadata {
            publish_intent_size: Some(3),
            ..ResumeMetadata::default()
        };
        for (partial, existing, recovered) in [
            (None, Some(&b"new"[..]), Some(3)),
            (Some(&b"new"[..]), Some(&b"new"[..]), Some(3)),
            (Some(&b"new"[..]), Some(&b"old"[..]), None),
            (None, Some(&b"user!"[..]), None),
            (Some(&b"new"[..]), None, None),
        ] {
            let _ = std::fs::remove_file(&staging.partial);
            let _ = std::fs::remove_file(&destination);
            if let Some(bytes) = partial {
                std::fs::write(&staging.partial, bytes).expect("partial");
            }
            if let Some(bytes) = existing {
                std::fs::write(&destination, bytes).expect("destination");
            }
            write_resume_metadata(&staging.metadata, &intent).expect("intent");

            let result = recover_publication(&staging, &destination).expect("recovery");

            assert_eq!(result, recovered, "{partial:?} {existing:?}");
            if let Some(bytes) = existing {
                assert_eq!(std::fs::read(&destination).expect("destination"), bytes);
            }
        }
    }

    async fn ranged_server(body: Arc<Vec<u8>>) -> (String, Arc<parking_lot::Mutex<Vec<u64>>>) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        let starts = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let recorded = starts.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let (body, starts) = (body.clone(), starts.clone());
                tokio::spawn(async move {
                    let mut request = vec![0_u8; 4096];
                    let read = socket.read(&mut request).await.expect("request");
                    let request = String::from_utf8_lossy(&request[..read]).to_ascii_lowercase();
                    let range = request.split("range: bytes=").nth(1).expect("ranged");
                    let (start, end) = range
                        .split_once("\r\n")
                        .expect("line")
                        .0
                        .split_once('-')
                        .expect("range");
                    let start: u64 = start.parse().expect("start");
                    let end = end.parse().unwrap_or(body.len() as u64 - 1);
                    if end != 1 {
                        starts.lock().push(start);
                    }
                    let head = format!(
                        "HTTP/1.1 206 Partial Content\r\nETag: \"v1\"\r\nContent-Range: bytes {start}-{end}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len(),
                        end - start + 1
                    );
                    socket.write_all(head.as_bytes()).await.expect("head");
                    let _ = socket.write_all(&body[start as usize..=end as usize]).await;
                });
            }
        });
        (format!("http://{address}/file.bin"), recorded)
    }

    #[tokio::test]
    async fn resume_fetches_only_the_missing_tails() {
        let total = 9_000_003_u64;
        let body: Arc<Vec<u8>> = Arc::new((0..total).map(|i| (i % 251) as u8).collect());
        let (url, starts) = ranged_server(body.clone()).await;
        let temp = tempfile::tempdir().expect("tempdir");
        let db = Arc::new(Database::open(&temp.path().join("shio.db")).expect("db"));
        let mut download = Download::new(url.clone(), temp.path().to_owned());
        download.filename = "file.bin".into();
        db.insert_download(&download).expect("row");
        ensure_owned_staging(&download).expect("staging");
        let staging = Staging::of(&download);

        let size = total / 8;
        let chunks: Vec<ChunkInfo> = (0..8_u32)
            .map(|index| {
                let start = u64::from(index) * size;
                let end = if index == 7 {
                    total - 1
                } else {
                    start + size - 1
                };
                ChunkInfo {
                    index,
                    start,
                    end,
                    downloaded: if index == 0 {
                        size
                    } else {
                        u64::from(index) * 1000
                    },
                    status: crate::types::ChunkStatus::Downloading,
                }
            })
            .collect();
        let mut partial = vec![0_u8; usize::try_from(chunks[7].start + 7000).expect("len")];
        for chunk in &chunks {
            let range = usize::try_from(chunk.start).expect("start")
                ..usize::try_from(chunk.start + chunk.downloaded).expect("end");
            partial[range.clone()].copy_from_slice(&body[range]);
        }
        std::fs::write(&staging.partial, partial).expect("partial");
        db.replace_chunks(download.id, &chunks).expect("chunks");
        let identity = ResumeMetadata {
            content_length: Some(total),
            validator: Some("\"v1\"".into()),
            ..ResumeMetadata::default()
        };
        write_resume_metadata(&staging.metadata, &identity).expect("metadata");

        let client = reqwest::Client::new();
        let probe = crate::probe::probe(&client, &url, &[])
            .await
            .expect("probe");
        let (progress_tx, _progress_rx) = mpsc::channel(64);
        let job = HttpJob {
            config: watch::channel(AppConfig::default()).1,
            client,
            db,
            progress_tx,
            cancel: CancellationToken::new(),
            limiter: BandwidthLimiter::new(None),
            host: Arc::new(Semaphore::new(16)),
        };
        let http = download.http().expect("http").clone();

        let published = download_payload(&download, &http, &probe, &job)
            .await
            .expect("download");

        assert_eq!(published, total);
        assert_eq!(std::fs::read(download.file_path()).expect("file"), *body);
        let mut starts = starts.lock().clone();
        starts.sort_unstable();
        let resumed: Vec<u64> = chunks[1..]
            .iter()
            .map(|chunk| chunk.start + chunk.downloaded)
            .collect();
        assert_eq!(starts, resumed, "only the missing tails are fetched");
    }

    #[test]
    fn oversized_resume_metadata_is_ignored() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("resume.json");
        std::fs::write(&path, vec![b' '; MAX_RESUME_METADATA_BYTES as usize + 1]).expect("write");

        assert_eq!(read_resume_metadata(&path), None);
    }

    #[test]
    fn staging_adopts_only_its_own_empty_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let download = download_in(temp.path(), "payload.bin");
        let staging = Staging::of(&download);
        std::fs::create_dir(&staging.root).expect("crashed root");

        ensure_owned_staging(&download).expect("empty root is adopted");
        ensure_owned_staging(&download).expect("owned root is reused");

        std::fs::write(staging.root.join("owner"), b"someone-else").expect("owner");
        assert!(matches!(
            ensure_owned_staging(&download),
            Err(ShioError::DestinationExists(_))
        ));
        std::fs::remove_file(staging.root.join("owner")).expect("remove owner");
        std::fs::write(&staging.partial, b"data").expect("payload");
        assert!(matches!(
            ensure_owned_staging(&download),
            Err(ShioError::DestinationExists(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn staging_refuses_symlinked_directories() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside");
        let mut download = download_in(temp.path(), "payload.bin");
        symlink(outside.path(), http_staging_root(&download)).expect("symlink root");
        assert!(matches!(
            ensure_owned_staging(&download),
            Err(ShioError::DestinationExists(_))
        ));

        symlink(outside.path(), temp.path().join("release")).expect("symlink subfolder");
        download.http_mut().expect("http").subfolder = Some("release".into());
        assert!(matches!(
            ensure_output_directory(&download),
            Err(ShioError::DestinationExists(_))
        ));
        assert!(
            std::fs::read_dir(outside.path())
                .expect("outside")
                .next()
                .is_none()
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn extraction_ignores_and_keeps_case_variant_user_archives() {
        let temp = tempfile::tempdir().expect("tempdir");
        let download = download_in(temp.path(), "photos.zip");
        crate::extract::tests::write_zip(&download.file_path(), &[("ours.txt", b"ours")]);
        let user = temp.path().join("Photos.zip");
        crate::extract::tests::write_zip(&user, &[("theirs.txt", b"theirs")]);
        let config = AppConfig {
            delete_archive_after_extract: true,
            ..AppConfig::default()
        };

        run_extract(
            std::slice::from_ref(&download),
            &config,
            None,
            CancellationToken::new(),
        )
        .await
        .expect("extract");

        assert_eq!(
            std::fs::read(temp.path().join("photos/ours.txt")).expect("extracted"),
            b"ours"
        );
        assert!(!temp.path().join("photos/theirs.txt").exists());
        assert!(!download.file_path().exists());
        assert!(user.exists());
    }
}
