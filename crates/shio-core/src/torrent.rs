mod selection;
mod session;

use std::path::Path;
use std::sync::Arc;

use self::selection::{
    selected_bytes, selected_file_indexes, selected_size, snapshot_with_selection, upload_ratio,
};
pub(crate) use self::session::{TorrentSessionStop, open_session, stop_torrent_in_session};
use librqbit::Session;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::error::{Result, ShioError};
use crate::types::{
    Download, DownloadId, DownloadProgress, DownloadStatus, MagnetPreviewManifest, ProgressDetail,
    TorrentFile, TorrentProgressSnapshot, TorrentSource,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum SeedPolicy {
    StopAtRatio { ratio: f32 },
    StopAtTime { seconds: u64 },
    RatioOrTime { ratio: f32, seconds: u64 },
    SeedForever,
    NeverSeed,
}

impl Default for SeedPolicy {
    fn default() -> Self {
        Self::RatioOrTime {
            ratio: 1.0,
            seconds: 7 * 24 * 60 * 60,
        }
    }
}

impl SeedPolicy {
    pub fn validate(self) -> Result<Self> {
        fn validate_ratio(ratio: f32) -> Result<()> {
            if ratio.is_finite() && ratio >= 0.0 {
                return Ok(());
            }
            Err(ShioError::Other(
                "seed ratio must be finite and non-negative".into(),
            ))
        }

        match self {
            Self::StopAtRatio { ratio } | Self::RatioOrTime { ratio, .. } => {
                validate_ratio(ratio)?;
            },
            Self::StopAtTime { .. } | Self::SeedForever | Self::NeverSeed => {},
        }
        Ok(self)
    }

    pub fn should_stop(&self, ratio: f32, seed_elapsed_secs: u64) -> bool {
        match self {
            Self::StopAtRatio { ratio: r } => ratio >= *r,
            Self::StopAtTime { seconds: s } => seed_elapsed_secs >= *s,
            Self::RatioOrTime {
                ratio: r,
                seconds: s,
            } => ratio >= *r || seed_elapsed_secs >= *s,
            Self::SeedForever => false,
            Self::NeverSeed => true,
        }
    }
}

pub(crate) struct TorrentWorkerDeps {
    pub(crate) session: Arc<Session>,
    pub(crate) progress_tx: mpsc::Sender<DownloadProgress>,
    pub(crate) cancel: CancellationToken,
    pub(crate) seed_policy: SeedPolicy,
    pub(crate) config: crate::config::AppConfig,
    pub(crate) on_download_phase_finished: Box<dyn FnOnce() + Send + 'static>,
}

impl std::fmt::Debug for TorrentWorkerDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TorrentWorkerDeps")
            .field("seed_policy", &self.seed_policy)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentManifest {
    pub info_hash: [u8; 20],
    pub name: String,
    pub total_size: u64,
    pub is_private: bool,
    pub files: Vec<TorrentFile>,
    pub trackers: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TorrentWorkerExit {
    Completed,
    SeedingStopped,
    Paused,
    Cancelled,
}

pub(crate) async fn run_torrent(
    mut download: Download,
    deps: TorrentWorkerDeps,
) -> Result<TorrentWorkerExit> {
    let id = download.id;
    let torrent = download
        .torrent()
        .ok_or_else(|| ShioError::Other("run_torrent: expected torrent kind".into()))?
        .clone();

    let add = add_torrent_from_state(&torrent);
    let opts = librqbit::AddTorrentOptions {
        overwrite: true,
        paused: false,
        only_files: selected_file_indexes(&torrent.files)?,
        output_folder: Some(
            crate::path::path_to_utf8("downloads.save_path", &download.save_path)?.to_string(),
        ),
        ..Default::default()
    };

    let response = deps
        .session
        .add_torrent(add, Some(opts))
        .await
        .map_err(|e| ShioError::Other(format!("add_torrent: {e}")))?;
    let handle = response
        .into_handle()
        .ok_or_else(|| ShioError::Other("add_torrent returned list-only".into()))?;

    emit_status(
        &deps.progress_tx,
        id,
        DownloadStatus::FetchingMetadata,
        download.downloaded,
        download.total_size,
    )
    .await;

    if !wait_for_torrent_metadata(&handle, id, &deps.progress_tx, &deps.cancel).await? {
        return Ok(TorrentWorkerExit::Cancelled);
    }

    let initial_stats = handle.stats();
    let (resolved_name, snapshot) = build_torrent_snapshot(&handle, &initial_stats)?;
    let snapshot = snapshot_with_selection(&snapshot, &torrent.files);
    apply_torrent_snapshot(&mut download, &resolved_name, &snapshot);
    emit_torrent_metadata(
        &deps.progress_tx,
        download.id,
        download.total_size.or(Some(initial_stats.total_bytes)),
        &resolved_name,
        snapshot,
        DownloadStatus::FetchingMetadata,
    )
    .await;

    if phase_after_metadata(TorrentWorkerCancellation::from_token(&deps.cancel))
        == TorrentWorkerPhase::Paused
    {
        return Ok(TorrentWorkerExit::Paused);
    }

    if handle.is_paused() {
        deps.session
            .unpause(&handle)
            .await
            .map_err(|e| ShioError::Other(format!("unpause: {e}")))?;
    }

    emit_status(
        &deps.progress_tx,
        id,
        DownloadStatus::Downloading,
        download.downloaded,
        download.total_size,
    )
    .await;
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(500));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            () = deps.cancel.cancelled() => return Ok(TorrentWorkerExit::Cancelled),
            _ = tick.tick() => {
                let stats = handle.stats();
                emit_torrent_progress(
                    &deps.progress_tx,
                    id,
                    &stats,
                    DownloadStatus::Downloading,
                    0,
                    None,
                    download.torrent().map(|torrent| torrent.files.as_slice()),
                )
                .await;
                if matches!(stats.state, librqbit::TorrentStatsState::Error) {
                    return Err(ShioError::Other(
                        stats.error.unwrap_or_else(|| "torrent error".into()),
                    ));
                }
                if stats.finished { break; }
            }
        }
    }

    if torrent.auto_extract && crate::worker::extraction_ready(&download).await {
        emit_status(
            &deps.progress_tx,
            id,
            DownloadStatus::Extracting,
            download.downloaded,
            download.total_size,
        )
        .await;
        let extract_result = crate::worker::run_extract(&download, &deps.config, None).await;
        let status = crate::worker::status_for_extract_result(&extract_result);
        emit_status(
            &deps.progress_tx,
            id,
            status,
            download.downloaded,
            download.total_size,
        )
        .await;
        extract_result?;
    }

    let persisted_seed_elapsed = torrent.seed_elapsed_secs;
    let selected_bytes = download
        .torrent()
        .map_or(0, |torrent| selected_bytes(&torrent.files));
    (deps.on_download_phase_finished)();

    if matches!(deps.seed_policy, SeedPolicy::NeverSeed) {
        stop_torrent_in_session(
            &deps.session,
            torrent.info_hash,
            TorrentSessionStop::KeepFiles,
        )
        .await?;
        emit_torrent_progress(
            &deps.progress_tx,
            id,
            &handle.stats(),
            DownloadStatus::Completed,
            persisted_seed_elapsed,
            Some(selected_bytes),
            download.torrent().map(|torrent| torrent.files.as_slice()),
        )
        .await;
        return Ok(TorrentWorkerExit::Completed);
    }

    emit_status(
        &deps.progress_tx,
        id,
        DownloadStatus::Seeding,
        download.downloaded,
        download.total_size,
    )
    .await;

    let seed_start = std::time::Instant::now();
    let mut seed_tick = tokio::time::interval(std::time::Duration::from_secs(2));
    seed_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            () = deps.cancel.cancelled() => {
                let stats = handle.stats();
                let seed_elapsed = accumulated_seed_elapsed_secs(persisted_seed_elapsed, seed_start);
                emit_torrent_progress(
                    &deps.progress_tx,
                    id,
                    &stats,
                    DownloadStatus::Paused,
                    seed_elapsed,
                    Some(selected_bytes),
                    download.torrent().map(|torrent| torrent.files.as_slice()),
                )
                .await;
                return Ok(TorrentWorkerExit::Paused);
            },
            _ = seed_tick.tick() => {
                let stats = handle.stats();
                let seed_elapsed = accumulated_seed_elapsed_secs(persisted_seed_elapsed, seed_start);
                let ratio = upload_ratio(stats.uploaded_bytes, selected_bytes);
                emit_torrent_progress(
                    &deps.progress_tx,
                    id,
                    &stats,
                    DownloadStatus::Seeding,
                    seed_elapsed,
                    Some(selected_bytes),
                    download.torrent().map(|torrent| torrent.files.as_slice()),
                )
                .await;
                if deps.seed_policy.should_stop(ratio, seed_elapsed) {
                    stop_torrent_in_session(
                        &deps.session,
                        torrent.info_hash,
                        TorrentSessionStop::KeepFiles,
                    )
                    .await?;
                    emit_torrent_progress(
                        &deps.progress_tx,
                        id,
                        &stats,
                        DownloadStatus::Completed,
                        seed_elapsed,
                        Some(selected_bytes),
                        download.torrent().map(|torrent| torrent.files.as_slice()),
                    )
                    .await;
                    return Ok(TorrentWorkerExit::SeedingStopped);
                }
            }
        }
    }
}

fn mbps_to_bps(mbps: f64) -> u64 {
    (mbps * 1024.0 * 1024.0) as u64
}

fn compute_ratio(stats: &librqbit::TorrentStats) -> f32 {
    upload_ratio(stats.uploaded_bytes, stats.progress_bytes)
}

fn add_torrent_from_state(torrent: &crate::types::TorrentState) -> librqbit::AddTorrent<'_> {
    match &torrent.source {
        TorrentSource::File(bytes) => librqbit::AddTorrent::from_bytes(bytes.clone()),
        TorrentSource::Magnet(magnet) => torrent.metadata_bytes.as_ref().map_or_else(
            || librqbit::AddTorrent::from_url(magnet.clone()),
            |bytes| librqbit::AddTorrent::from_bytes(bytes.clone()),
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TorrentWorkerCancellation {
    Requested,
    Clear,
}

impl TorrentWorkerCancellation {
    fn from_token(token: &CancellationToken) -> Self {
        if token.is_cancelled() {
            Self::Requested
        } else {
            Self::Clear
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TorrentWorkerPhase {
    Downloading,
    Paused,
}

const fn phase_after_metadata(cancellation: TorrentWorkerCancellation) -> TorrentWorkerPhase {
    match cancellation {
        TorrentWorkerCancellation::Requested => TorrentWorkerPhase::Paused,
        TorrentWorkerCancellation::Clear => TorrentWorkerPhase::Downloading,
    }
}

async fn wait_for_torrent_metadata(
    handle: &librqbit::ManagedTorrent,
    id: DownloadId,
    progress_tx: &mpsc::Sender<DownloadProgress>,
    cancel: &CancellationToken,
) -> Result<bool> {
    let init = handle.wait_until_initialized();
    tokio::pin!(init);
    let metadata_started = std::time::Instant::now();
    let mut metadata_tick = tokio::time::interval(std::time::Duration::from_secs(5));
    metadata_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            res = &mut init => {
                res.map_err(|e| ShioError::Other(format!("metadata init: {e}")))?;
                return Ok(true);
            }
            _ = metadata_tick.tick() => {
                emit_torrent_wait(
                    progress_tx,
                    id,
                    DownloadStatus::FetchingMetadata,
                    metadata_started.elapsed().as_secs(),
                )
                .await;
            }
            () = cancel.cancelled() => {
                return Ok(false);
            }
        }
    }
}

fn build_torrent_snapshot(
    handle: &librqbit::ManagedTorrent,
    stats: &librqbit::TorrentStats,
) -> Result<(String, TorrentProgressSnapshot)> {
    let metadata: Result<(String, bool, Vec<TorrentFile>, Vec<String>)> = handle
        .with_metadata(|metadata| {
            let selected = handle.only_files();
            let files = metadata
                .file_infos
                .iter()
                .enumerate()
                .map(|(index, file)| {
                    validate_torrent_file_path(&file.relative_filename)?;
                    Ok(TorrentFile {
                        path: file.relative_filename.clone(),
                        size: file.len,
                        downloaded: stats.file_progress.get(index).copied().unwrap_or(0),
                        selected: selected
                            .as_ref()
                            .is_none_or(|selected| selected.contains(&index)),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let trackers = handle
                .shared()
                .trackers
                .iter()
                .map(url::Url::to_string)
                .collect::<Vec<_>>();
            let name = metadata
                .name
                .clone()
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "torrent".to_string());
            let is_private = metadata.info.private;
            Ok((name, is_private, files, trackers))
        })
        .map_err(|e| ShioError::Other(format!("torrent metadata: {e}")))?;

    let (name, is_private, files, trackers) = metadata?;
    Ok((
        crate::sanitize_filename(&name),
        TorrentProgressSnapshot {
            is_private,
            files,
            trackers,
        },
    ))
}

fn apply_torrent_snapshot(
    download: &mut Download,
    filename: &str,
    snapshot: &TorrentProgressSnapshot,
) {
    download.filename = filename.to_string();
    download.downloaded = selected_downloaded(&snapshot.files);
    download.total_size = Some(selected_size(&snapshot.files));
    if let Some(torrent) = download.torrent_mut() {
        torrent.is_private = snapshot.is_private;
        torrent.files.clone_from(&snapshot.files);
        torrent.trackers.clone_from(&snapshot.trackers);
    }
}

fn selected_downloaded(files: &[TorrentFile]) -> u64 {
    files
        .iter()
        .filter(|file| file.selected)
        .map(|file| file.downloaded)
        .sum()
}

fn selected_progress(files: &[TorrentFile], file_progress: &[u64]) -> u64 {
    files
        .iter()
        .zip(file_progress)
        .filter(|(file, _)| file.selected)
        .map(|(_, downloaded)| *downloaded)
        .sum()
}

async fn emit_status(
    tx: &mpsc::Sender<DownloadProgress>,
    id: DownloadId,
    status: DownloadStatus,
    downloaded: u64,
    total_size: Option<u64>,
) {
    let _ = tx
        .send(status_progress(id, status, downloaded, total_size))
        .await;
}

const fn status_progress(
    id: DownloadId,
    status: DownloadStatus,
    downloaded: u64,
    total_size: Option<u64>,
) -> DownloadProgress {
    DownloadProgress {
        id,
        downloaded,
        total_size,
        speed: 0,
        avg_speed: 0,
        status,
        detail: ProgressDetail::empty_torrent(),
        filename: None,
        torrent_snapshot: None,
    }
}

async fn emit_torrent_metadata(
    tx: &mpsc::Sender<DownloadProgress>,
    id: DownloadId,
    total_size: Option<u64>,
    filename: &str,
    snapshot: TorrentProgressSnapshot,
    status: DownloadStatus,
) {
    let _ = tx
        .send(DownloadProgress {
            id,
            downloaded: selected_downloaded(&snapshot.files),
            total_size,
            speed: 0,
            avg_speed: 0,
            status,
            detail: ProgressDetail::empty_torrent(),
            filename: Some(filename.to_string()),
            torrent_snapshot: Some(snapshot),
        })
        .await;
}

async fn emit_torrent_progress(
    tx: &mpsc::Sender<DownloadProgress>,
    id: DownloadId,
    stats: &librqbit::TorrentStats,
    status: DownloadStatus,
    seed_elapsed_secs: u64,
    ratio_bytes: Option<u64>,
    selected_files: Option<&[TorrentFile]>,
) {
    let live = stats.live.as_ref();
    let download_speed = live.map_or(0, |l| mbps_to_bps(l.download_speed.mbps));
    let upload_speed = live.map_or(0, |l| mbps_to_bps(l.upload_speed.mbps));
    let peers_connected = live.map_or(0, |l| l.snapshot.peer_stats.live as u32);
    let ratio = ratio_bytes.map_or_else(
        || compute_ratio(stats),
        |bytes| upload_ratio(stats.uploaded_bytes, bytes),
    );
    let primary_speed = if status == DownloadStatus::Seeding {
        upload_speed
    } else {
        download_speed
    };
    let downloaded = selected_files.map_or(stats.progress_bytes, |files| {
        selected_progress(files, &stats.file_progress)
    });
    let total_size = selected_files.map_or(stats.total_bytes, selected_bytes);

    let _ = tx
        .send(DownloadProgress {
            id,
            downloaded,
            total_size: Some(total_size),
            speed: primary_speed,
            avg_speed: primary_speed,
            status,
            detail: ProgressDetail::Torrent {
                peers_connected,
                seeders: 0,
                leechers: 0,
                uploaded: stats.uploaded_bytes,
                upload_speed,
                ratio,
                seed_elapsed_secs,
                metadata_wait_secs: 0,
            },
            filename: None,
            torrent_snapshot: None,
        })
        .await;
}

async fn emit_torrent_wait(
    tx: &mpsc::Sender<DownloadProgress>,
    id: DownloadId,
    status: DownloadStatus,
    metadata_wait_secs: u64,
) {
    let _ = tx
        .send(DownloadProgress {
            id,
            downloaded: 0,
            total_size: None,
            speed: 0,
            avg_speed: 0,
            status,
            detail: ProgressDetail::Torrent {
                peers_connected: 0,
                seeders: 0,
                leechers: 0,
                uploaded: 0,
                upload_speed: 0,
                ratio: 0.0,
                seed_elapsed_secs: 0,
                metadata_wait_secs,
            },
            filename: None,
            torrent_snapshot: None,
        })
        .await;
}

pub(crate) fn parse_magnet_info_hash(magnet: &str) -> Result<[u8; 20]> {
    let m = librqbit::Magnet::parse(magnet)
        .map_err(|e| ShioError::Other(format!("invalid magnet: {e}")))?;
    m.as_id20()
        .map(|h| h.0)
        .ok_or_else(|| ShioError::Other("magnet missing v1 info hash".into()))
}

pub fn parse_torrent_manifest(bytes: &[u8]) -> Result<TorrentManifest> {
    let meta: librqbit::TorrentMetaV1Owned = librqbit::torrent_from_bytes(bytes)
        .map_err(|e| ShioError::Other(format!("invalid .torrent: {e}")))?;
    let name = meta
        .info
        .name
        .as_ref()
        .and_then(|name| std::str::from_utf8(name.as_ref()).ok())
        .filter(|name| !name.is_empty())
        .map_or_else(|| "torrent".to_string(), crate::sanitize_filename);
    let files = meta
        .info
        .iter_file_details()
        .map_err(|e| ShioError::Other(format!("torrent files: {e}")))?
        .map(|file| {
            let path = file
                .filename
                .to_pathbuf()
                .map_err(|e| ShioError::Other(format!("torrent file path: {e}")))?;
            validate_torrent_file_path(&path)?;
            Ok(TorrentFile {
                path,
                size: file.len,
                downloaded: 0,
                selected: true,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let trackers = meta
        .iter_announce()
        .filter_map(|tracker| std::str::from_utf8(tracker.as_ref()).ok())
        .filter(|tracker| !tracker.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let total_size = files.iter().map(|file| file.size).sum();

    Ok(TorrentManifest {
        info_hash: meta.info_hash.0,
        name,
        total_size,
        is_private: meta.info.private,
        files,
        trackers,
    })
}

pub(crate) fn magnet_preview_manifest_from_bytes(bytes: &[u8]) -> Result<MagnetPreviewManifest> {
    let manifest = parse_torrent_manifest(bytes)?;
    Ok(MagnetPreviewManifest {
        name: manifest.name,
        total_size: manifest.total_size,
        is_private: manifest.is_private,
        files: manifest.files,
        trackers: manifest.trackers,
        metadata_bytes: bytes.to_vec(),
    })
}

pub(crate) async fn resolve_magnet_preview(
    session: &Arc<Session>,
    magnet: &str,
) -> Result<MagnetPreviewManifest> {
    let response = session
        .add_torrent(
            librqbit::AddTorrent::from_url(magnet),
            Some(librqbit::AddTorrentOptions {
                list_only: true,
                ..Default::default()
            }),
        )
        .await
        .map_err(|e| ShioError::Other(format!("magnet metadata: {e}")))?;

    let torrent_bytes = match response {
        librqbit::AddTorrentResponse::AlreadyManaged(_, handle) => handle
            .with_metadata(|metadata| metadata.torrent_bytes.clone())
            .map_err(|e| ShioError::Other(format!("magnet metadata: {e}")))?,
        librqbit::AddTorrentResponse::ListOnly(librqbit::ListOnlyResponse {
            torrent_bytes,
            ..
        }) => torrent_bytes,
        librqbit::AddTorrentResponse::Added(_, _) => {
            return Err(ShioError::Other(
                "magnet preview unexpectedly started a download".into(),
            ));
        },
    };

    magnet_preview_manifest_from_bytes(&torrent_bytes)
}

fn validate_torrent_file_path(path: &Path) -> Result<()> {
    crate::path::validate_relative_path(path).map_err(|e| ShioError::Other(e.to_string()))
}

fn accumulated_seed_elapsed_secs(persisted_secs: u64, seed_started_at: std::time::Instant) -> u64 {
    persisted_secs.saturating_add(seed_started_at.elapsed().as_secs())
}

#[cfg(test)]
mod tests {
    use self::session::listen_port_range;
    use super::*;
    use librqbit::SessionOptions;

    const UBUNTU_MAGNET: &str =
        "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862&dn=ubuntu";

    fn sample_torrent_bytes() -> Vec<u8> {
        let pieces = b"abcdefghijklmnopqrst";
        let mut bytes = b"d8:announce35:udp://tracker.example:1337/announce4:infod5:filesld6:lengthi123e4:pathl7:foo.txteed6:lengthi456e4:pathl3:bar7:baz.mkveee4:name7:release12:piece lengthi16384e6:pieces20:".to_vec();
        bytes.extend_from_slice(pieces);
        bytes.extend_from_slice(b"ee");
        bytes
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn torrent_file_with_known_files_reaches_metadata_progress() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let session = Session::new_with_opts(
            tmp.path().join("rqbit-output"),
            SessionOptions {
                disable_dht: true,
                persistence: None,
                listen_port_range: None,
                enable_upnp_port_forwarding: false,
                ..Default::default()
            },
        )
        .await
        .expect("session");
        let (progress_tx, mut progress_rx) = mpsc::channel(8);
        let cancel = CancellationToken::new();
        let mut download = Download::try_from_torrent(
            TorrentSource::File(sample_torrent_bytes()),
            tmp.path().join("downloads"),
        )
        .expect("valid torrent fixture");
        download.status = DownloadStatus::Queued;

        let deps = TorrentWorkerDeps {
            session,
            progress_tx,
            cancel: cancel.clone(),
            seed_policy: SeedPolicy::NeverSeed,
            config: crate::config::AppConfig::default(),
            on_download_phase_finished: Box::new(|| {}),
        };

        let task = tokio::spawn(run_torrent(download, deps));
        let saw_metadata = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while let Some(progress) = progress_rx.recv().await {
                if progress.status == DownloadStatus::FetchingMetadata
                    && progress.torrent_snapshot.is_some()
                {
                    return true;
                }
            }
            false
        })
        .await
        .expect("metadata timeout");
        cancel.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), task).await;

        assert!(saw_metadata);
    }

    #[test]
    fn parses_valid_magnet_info_hash() {
        let h = parse_magnet_info_hash(UBUNTU_MAGNET).expect("parse");
        assert_eq!(h.len(), 20);
    }

    #[test]
    fn rejects_invalid_magnet() {
        assert!(parse_magnet_info_hash("not a magnet").is_err());
    }

    #[test]
    fn rejects_invalid_torrent_bytes() {
        assert!(parse_torrent_manifest(&[0u8; 16]).is_err());
    }

    #[test]
    fn parses_torrent_manifest_files_and_trackers() {
        let manifest = parse_torrent_manifest(&sample_torrent_bytes()).expect("manifest");

        assert_eq!(manifest.name, "release");
        assert_eq!(manifest.total_size, 579);
        assert_eq!(manifest.files.len(), 2);
        assert_eq!(manifest.files[0].path, std::path::PathBuf::from("foo.txt"));
        assert_eq!(
            manifest.files[1].path,
            std::path::PathBuf::from("bar/baz.mkv")
        );
        assert!(manifest.files.iter().all(|file| file.selected));
        assert_eq!(
            manifest.trackers,
            vec!["udp://tracker.example:1337/announce".to_string()]
        );
    }

    #[test]
    fn torrent_path_validation_rejects_unsafe_paths() {
        for path in [
            std::path::PathBuf::from("../evil.bin"),
            std::path::PathBuf::from("/tmp/evil.bin"),
            std::path::PathBuf::from("release/../evil.bin"),
            std::path::PathBuf::from(""),
            std::path::PathBuf::from("CON"),
            std::path::PathBuf::from("folder/NUL.txt"),
        ] {
            assert!(validate_torrent_file_path(&path).is_err(), "{path:?}");
        }
    }

    #[test]
    fn torrent_path_validation_accepts_nested_relative_paths() {
        assert!(validate_torrent_file_path(std::path::Path::new("release/cd1/file.bin")).is_ok());
    }

    #[test]
    fn magnet_preview_manifest_uses_torrent_metadata() {
        let manifest =
            magnet_preview_manifest_from_bytes(&sample_torrent_bytes()).expect("preview manifest");

        assert_eq!(manifest.name, "release");
        assert_eq!(manifest.total_size, 579);
        assert_eq!(manifest.files.len(), 2);
        assert!(manifest.files.iter().all(|file| file.selected));
        assert_eq!(
            manifest.trackers,
            vec!["udp://tracker.example:1337/announce".to_string()]
        );
    }

    #[test]
    fn magnet_preview_manifest_retains_metadata_bytes() {
        let bytes = sample_torrent_bytes();
        let manifest = magnet_preview_manifest_from_bytes(&bytes).expect("preview manifest");

        assert_eq!(manifest.metadata_bytes, bytes);
    }

    #[test]
    fn selected_file_indexes_returns_only_unchecked_filter() {
        let mut manifest = parse_torrent_manifest(&sample_torrent_bytes()).expect("manifest");
        assert_eq!(selected_file_indexes(&manifest.files).unwrap(), None);

        manifest.files[0].selected = false;

        assert_eq!(
            selected_file_indexes(&manifest.files).unwrap(),
            Some(vec![1])
        );
    }

    #[test]
    fn selected_file_indexes_rejects_empty_selection() {
        let mut manifest = parse_torrent_manifest(&sample_torrent_bytes()).expect("manifest");
        for file in &mut manifest.files {
            file.selected = false;
        }

        assert!(selected_file_indexes(&manifest.files).is_err());
    }

    #[test]
    fn snapshot_keeps_existing_deselected_files() {
        let mut download = Download::try_from_torrent(
            TorrentSource::File(sample_torrent_bytes()),
            std::path::PathBuf::from("/tmp"),
        )
        .expect("valid torrent fixture");
        download
            .torrent_mut()
            .expect("torrent")
            .files
            .get_mut(0)
            .expect("file")
            .selected = false;

        let snapshot = TorrentProgressSnapshot {
            is_private: false,
            files: vec![
                TorrentFile {
                    path: std::path::PathBuf::from("foo.txt"),
                    size: 123,
                    downloaded: 0,
                    selected: true,
                },
                TorrentFile {
                    path: std::path::PathBuf::from("bar/baz.mkv"),
                    size: 456,
                    downloaded: 0,
                    selected: true,
                },
            ],
            trackers: Vec::new(),
        };

        let snapshot =
            snapshot_with_selection(&snapshot, &download.torrent().expect("torrent").files);
        apply_torrent_snapshot(&mut download, "release", &snapshot);

        let files = &download.torrent().expect("torrent").files;
        assert!(!files[0].selected);
        assert_eq!(download.total_size, Some(456));
    }

    #[test]
    fn metadata_snapshot_preserves_requested_selection() {
        let existing = vec![
            TorrentFile {
                path: std::path::PathBuf::from("foo.txt"),
                size: 123,
                downloaded: 0,
                selected: false,
            },
            TorrentFile {
                path: std::path::PathBuf::from("bar/baz.mkv"),
                size: 456,
                downloaded: 0,
                selected: true,
            },
        ];
        let snapshot = TorrentProgressSnapshot {
            is_private: false,
            files: vec![
                TorrentFile {
                    path: std::path::PathBuf::from("foo.txt"),
                    size: 123,
                    downloaded: 7,
                    selected: true,
                },
                TorrentFile {
                    path: std::path::PathBuf::from("bar/baz.mkv"),
                    size: 456,
                    downloaded: 9,
                    selected: true,
                },
            ],
            trackers: Vec::new(),
        };

        let preserved = snapshot_with_selection(&snapshot, &existing);

        assert!(!preserved.files[0].selected);
        assert_eq!(preserved.files[1].downloaded, 9);
    }

    #[test]
    fn listen_port_range_clamps_high_ports() {
        assert_eq!(listen_port_range(6881), 6881..6891);
        assert_eq!(listen_port_range(u16::MAX), 65525..65535);
    }

    #[test]
    fn seed_policy_should_stop_for_each_mode() {
        let cases = [
            (SeedPolicy::StopAtRatio { ratio: 1.0 }, 0.9, 999_999, false),
            (SeedPolicy::StopAtRatio { ratio: 1.0 }, 1.0, 0, true),
            (SeedPolicy::StopAtTime { seconds: 60 }, 99.0, 59, false),
            (SeedPolicy::StopAtTime { seconds: 60 }, 0.0, 60, true),
            (
                SeedPolicy::RatioOrTime {
                    ratio: 1.0,
                    seconds: 60,
                },
                0.5,
                30,
                false,
            ),
            (
                SeedPolicy::RatioOrTime {
                    ratio: 1.0,
                    seconds: 60,
                },
                1.0,
                0,
                true,
            ),
            (
                SeedPolicy::RatioOrTime {
                    ratio: 1.0,
                    seconds: 60,
                },
                0.0,
                60,
                true,
            ),
            (SeedPolicy::SeedForever, 999.0, 999_999, false),
            (SeedPolicy::NeverSeed, 0.0, 0, true),
            (SeedPolicy::default(), 1.0, 0, true),
            (SeedPolicy::default(), 0.0, 7 * 24 * 60 * 60, true),
            (SeedPolicy::default(), 0.5, 60, false),
        ];

        for (policy, ratio, elapsed, expected) in cases {
            assert_eq!(policy.should_stop(ratio, elapsed), expected, "{policy:?}");
        }
    }

    #[test]
    fn seed_policy_validation_rejects_invalid_ratios_and_allows_zero_limits() {
        for policy in [
            SeedPolicy::StopAtRatio { ratio: f32::NAN },
            SeedPolicy::StopAtRatio {
                ratio: f32::INFINITY,
            },
            SeedPolicy::StopAtRatio { ratio: -0.1 },
            SeedPolicy::RatioOrTime {
                ratio: -0.1,
                seconds: 60,
            },
        ] {
            assert!(policy.validate().is_err(), "{policy:?}");
        }

        assert!(SeedPolicy::StopAtRatio { ratio: 0.0 }.validate().is_ok());
        assert!(SeedPolicy::StopAtTime { seconds: 0 }.validate().is_ok());
        assert!(
            SeedPolicy::RatioOrTime {
                ratio: 0.0,
                seconds: 0,
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn selected_bytes_sums_only_selected_files() {
        let files = vec![
            TorrentFile {
                path: std::path::PathBuf::from("a.bin"),
                size: 100,
                downloaded: 100,
                selected: true,
            },
            TorrentFile {
                path: std::path::PathBuf::from("b.bin"),
                size: 900,
                downloaded: 0,
                selected: false,
            },
        ];

        assert_eq!(selected_bytes(&files), 100);
    }

    #[test]
    fn upload_ratio_uses_selected_bytes() {
        assert_eq!(upload_ratio(50, 100), 0.5);
        assert_eq!(upload_ratio(100, 100), 1.0);
    }

    #[test]
    fn upload_ratio_is_zero_when_selected_bytes_unknown() {
        assert_eq!(upload_ratio(500, 0), 0.0);
    }

    #[test]
    fn accumulated_seed_elapsed_adds_persisted_offset() {
        let started = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(12))
            .expect("instant subtraction should be in range");

        assert!(accumulated_seed_elapsed_secs(300, started) >= 312);
    }

    #[test]
    fn accumulated_seed_elapsed_saturates_at_u64_max() {
        let started = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(12))
            .expect("instant subtraction should be in range");

        assert_eq!(accumulated_seed_elapsed_secs(u64::MAX, started), u64::MAX);
    }

    #[test]
    fn metadata_phase_does_not_resume_after_cancellation() {
        assert_eq!(
            phase_after_metadata(TorrentWorkerCancellation::Requested),
            TorrentWorkerPhase::Paused
        );
    }
}
