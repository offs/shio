mod metainfo;
mod session;
mod staging;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use librqbit::{
    AddTorrent, AddTorrentOptions, ManagedTorrent, ManagedTorrentState, Session, TorrentStatsState,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use self::metainfo::staged_files;
pub use self::metainfo::{MAX_TORRENT_METADATA_BYTES, TorrentManifest, parse_torrent_manifest};
pub(crate) use self::metainfo::{parse_magnet_info_hash, validate_torrent_state};
pub(crate) use self::session::{
    librqbit_rate_limit, open_session, resolve_magnet_preview, stop_torrent_in_session,
};
use self::session::{pause_torrent_in_session, resolve_magnet, torrent_id};
use self::staging::ensure_owned_staging;
pub(crate) use self::staging::{
    cleanup_staging, prepare_completed_recheck, published_files, staging_root,
};
use crate::config::AppConfig;
use crate::engine::{ActiveSlot, MetadataCommit};
use crate::error::{Result, ShioError};
use crate::extract::Readiness;
use crate::types::{
    Download, DownloadProgress, DownloadStatus, TorrentSource, TorrentState, TorrentStats,
};

const DOWNLOAD_TICK: Duration = Duration::from_millis(500);
const METADATA_TICK: Duration = Duration::from_secs(1);
const SEED_TICK: Duration = Duration::from_secs(2);

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
    pub(crate) fn validate(self) -> Result<()> {
        match self {
            Self::StopAtRatio { ratio } | Self::RatioOrTime { ratio, .. }
                if !ratio.is_finite() || ratio < 0.0 =>
            {
                Err(ShioError::Config(
                    "seed ratio must be finite and non-negative".into(),
                ))
            },
            _ => Ok(()),
        }
    }

    fn should_stop(self, ratio: f32, seed_elapsed_secs: u64) -> bool {
        match self {
            Self::StopAtRatio { ratio: limit } => ratio >= limit,
            Self::StopAtTime { seconds } => seed_elapsed_secs >= seconds,
            Self::RatioOrTime {
                ratio: limit,
                seconds,
            } => ratio >= limit || seed_elapsed_secs >= seconds,
            Self::SeedForever => false,
            Self::NeverSeed => true,
        }
    }
}

pub(crate) struct TorrentWorkerDeps {
    pub(crate) session: Arc<Session>,
    pub(crate) progress_tx: mpsc::Sender<DownloadProgress>,
    pub(crate) cancel: CancellationToken,
    pub(crate) config: watch::Receiver<AppConfig>,
    pub(crate) slot: ActiveSlot,
    pub(crate) metadata: MetadataCommit,
}

pub(crate) async fn run_torrent(download: Download, deps: TorrentWorkerDeps) -> Result<()> {
    let TorrentWorkerDeps {
        session,
        progress_tx,
        cancel,
        config,
        slot,
        metadata,
    } = deps;
    let state = download
        .torrent()
        .ok_or_else(|| ShioError::Other("expected a torrent download".into()))?;
    let mut worker = Worker {
        info_hash: state.info_hash,
        base: state.stats,
        last_uploaded: 0,
        selected: Vec::new(),
        selected_bytes: 0,
        download,
        session,
        progress_tx,
        cancel,
        config,
    };
    let result = worker.run(slot, &metadata).await;
    if result.is_err()
        && let Err(error) = pause_torrent_in_session(&worker.session, worker.info_hash).await
    {
        tracing::warn!(download = %worker.download.id, "pause torrent after interrupt failed: {error}");
    }
    result
}

struct Worker {
    download: Download,
    info_hash: [u8; 20],
    session: Arc<Session>,
    progress_tx: mpsc::Sender<DownloadProgress>,
    cancel: CancellationToken,
    config: watch::Receiver<AppConfig>,
    /// Counters from earlier runs; librqbit restarts its own in every session.
    base: TorrentStats,
    last_uploaded: u64,
    selected: Vec<(usize, u64)>,
    selected_bytes: u64,
}

impl Worker {
    const fn torrent(&self) -> &TorrentState {
        self.download
            .torrent()
            .expect("run_torrent only builds a worker for torrent downloads")
    }

    async fn run(&mut self, slot: ActiveSlot, metadata: &MetadataCommit) -> Result<()> {
        let bytes = self.metadata(metadata).await?;
        let staged = staged_files(self.torrent(), &bytes)?;
        self.selected = self
            .torrent()
            .files
            .iter()
            .zip(&staged)
            .filter(|(file, _)| file.selected)
            .map(|(file, staged)| (staged.index, file.size))
            .collect();
        self.selected_bytes = self.selected.iter().map(|&(_, size)| size).sum();
        let everything = self.selected.len() == staged.len()
            && staged.iter().enumerate().all(|(i, file)| file.index == i);
        let only_files =
            (!everything).then(|| self.selected.iter().map(|&(index, _)| index).collect());

        let payload = ensure_owned_staging(&self.download).await?;
        let was_published = staging::is_published(&self.download).await?;
        if !was_published {
            staging::check_destinations(&self.download, &staged).await?;
        }
        let handle = self
            .until_cancelled(self.handle(bytes, &payload, only_files))
            .await??;
        self.download_phase(&handle).await?;
        if !was_published {
            let never_seeds = matches!(
                self.config.borrow().torrent.seed_policy,
                SeedPolicy::NeverSeed
            );
            staging::publish_payload(&self.download, &staged, never_seeds).await?;
            self.auto_extract(&handle).await?;
        }
        drop(slot);
        self.seed(&handle).await
    }

    /// Magnet metadata is validated and committed before librqbit sees it, so unvalidated
    /// metadata never reaches the disk.
    async fn metadata(&mut self, commit: &MetadataCommit) -> Result<Vec<u8>> {
        let state = self.torrent();
        let magnet = match (&state.source, &state.metadata_bytes) {
            (TorrentSource::File(bytes), _) | (TorrentSource::Magnet(_), Some(bytes)) => {
                return Ok(bytes.clone());
            },
            (TorrentSource::Magnet(magnet), None) => magnet.clone(),
        };
        let bytes = self.resolve_metadata(&magnet).await?;
        let manifest = parse_torrent_manifest(&bytes)?;
        if let Some(state) = self.download.torrent_mut() {
            state.files.clone_from(&manifest.files);
        }
        commit.commit(manifest, bytes.clone()).await?;
        Ok(bytes)
    }

    async fn resolve_metadata(&self, magnet: &str) -> Result<Vec<u8>> {
        let started = Instant::now();
        let resolve = resolve_magnet(&self.session, magnet);
        tokio::pin!(resolve);
        loop {
            self.send(DownloadProgress {
                id: self.download.id,
                status: DownloadStatus::FetchingMetadata,
                downloaded: self.download.downloaded,
                total_size: self.download.total_size,
                speed: 0,
                torrent: Some(TorrentStats {
                    metadata_wait_secs: started.elapsed().as_secs(),
                    ..self.base
                }),
            })
            .await;
            tokio::select! {
                () = self.cancel.cancelled() => return Err(ShioError::Cancelled),
                bytes = &mut resolve => return bytes,
                () = tokio::time::sleep(METADATA_TICK) => {},
            }
        }
    }

    /// Reuses the session's entry when it matches this download, otherwise replaces it: librqbit
    /// answers a re-add with the existing entry and ignores the new folder and file selection.
    async fn handle(
        &self,
        bytes: Vec<u8>,
        output_folder: &Path,
        only_files: Option<Vec<usize>>,
    ) -> Result<Arc<ManagedTorrent>> {
        if let Some(handle) = self.session.get(torrent_id(self.info_hash)) {
            if handle.output_folder() == output_folder && handle.only_files() == only_files {
                let errored =
                    handle.with_state(|state| matches!(state, ManagedTorrentState::Error(_)));
                if handle.is_paused() || errored {
                    self.session
                        .unpause(&handle)
                        .await
                        .map_err(|e| ShioError::Other(format!("resume torrent: {e:#}")))?;
                }
                return Ok(handle);
            }
            stop_torrent_in_session(&self.session, self.info_hash).await?;
        }
        let output_folder = output_folder
            .to_str()
            .ok_or_else(|| ShioError::Other("torrent download folder is not valid UTF-8".into()))?;
        let options = AddTorrentOptions {
            // The staging folder belongs to this download, so librqbit may reuse its files.
            overwrite: true,
            only_files,
            output_folder: Some(output_folder.to_owned()),
            ..Default::default()
        };
        self.session
            .add_torrent(AddTorrent::from_bytes(bytes), Some(options))
            .await
            .map_err(|e| ShioError::Other(format!("add torrent: {e:#}")))?
            .into_handle()
            .ok_or_else(|| ShioError::Other("add torrent: librqbit returned no handle".into()))
    }

    async fn download_phase(&mut self, handle: &Arc<ManagedTorrent>) -> Result<()> {
        loop {
            let stats = handle.stats();
            self.check_state(handle, &stats).await?;
            self.send(self.progress(
                &stats,
                DownloadStatus::Downloading,
                self.base.seed_elapsed_secs,
            ))
            .await;
            if stats.finished {
                return Ok(());
            }
            self.until_cancelled(tokio::time::sleep(DOWNLOAD_TICK))
                .await?;
        }
    }

    async fn auto_extract(&self, handle: &ManagedTorrent) -> Result<()> {
        if !self.torrent().auto_extract {
            return Ok(());
        }
        match crate::worker::extraction_readiness(&self.download).await {
            None => return Ok(()),
            Some(Readiness::Ready) => {},
            Some(Readiness::PasswordRequired) => return Err(ShioError::PasswordRequired),
            Some(Readiness::Incomplete) => {
                return Err(ShioError::Extract("archive set is incomplete".into()));
            },
        }
        self.send(self.progress(
            &handle.stats(),
            DownloadStatus::Extracting,
            self.base.seed_elapsed_secs,
        ))
        .await;
        let config = self.config.borrow().clone();
        crate::worker::run_extract(
            std::slice::from_ref(&self.download),
            &config,
            None,
            self.cancel.clone(),
        )
        .await
    }

    async fn seed(&mut self, handle: &Arc<ManagedTorrent>) -> Result<()> {
        let started = Instant::now();
        let mut status = DownloadStatus::Downloading;
        loop {
            let stats = handle.stats();
            self.check_state(handle, &stats).await?;
            let seed_elapsed = self
                .base
                .seed_elapsed_secs
                .saturating_add(started.elapsed().as_secs());
            let uploaded = self.base.uploaded.saturating_add(stats.uploaded_bytes);
            let policy = self.config.borrow().torrent.seed_policy;
            if policy.should_stop(ratio(uploaded, self.selected_bytes), seed_elapsed) {
                self.send(self.progress(&stats, status, seed_elapsed)).await;
                self.leave_session().await;
                return Ok(());
            }
            status = DownloadStatus::Seeding;
            self.send(self.progress(&stats, status, seed_elapsed)).await;
            self.until_cancelled(tokio::time::sleep(SEED_TICK)).await?;
        }
    }

    /// The payload is published, so failing to leave the session must not fail the download.
    async fn leave_session(&self) {
        let left = match stop_torrent_in_session(&self.session, self.info_hash).await {
            Ok(()) => cleanup_staging(&self.download).await,
            Err(error) => {
                if let Err(pause_error) =
                    pause_torrent_in_session(&self.session, self.info_hash).await
                {
                    tracing::warn!(download = %self.download.id, "pause finished torrent failed: {pause_error}");
                }
                Err(error)
            },
        };
        if let Err(error) = left {
            tracing::warn!(download = %self.download.id, "finished torrent left session state behind: {error}");
        }
    }

    async fn check_state(
        &mut self,
        handle: &Arc<ManagedTorrent>,
        stats: &librqbit::TorrentStats,
    ) -> Result<()> {
        match stats.state {
            TorrentStatsState::Error => Err(ShioError::Other(format!(
                "torrent: {}",
                stats.error.as_deref().unwrap_or("unknown error")
            ))),
            // A pause from an earlier run of this download can land after this run resumed it.
            TorrentStatsState::Paused => {
                self.base.uploaded = self.base.uploaded.saturating_add(self.last_uploaded);
                self.last_uploaded = 0;
                self.session
                    .unpause(handle)
                    .await
                    .map_err(|e| ShioError::Other(format!("resume torrent: {e:#}")))
            },
            TorrentStatsState::Live => {
                self.last_uploaded = stats.uploaded_bytes;
                Ok(())
            },
            TorrentStatsState::Initializing { .. } => Ok(()),
        }
    }

    fn progress(
        &self,
        stats: &librqbit::TorrentStats,
        status: DownloadStatus,
        seed_elapsed_secs: u64,
    ) -> DownloadProgress {
        let live = stats.live.as_ref();
        let download_speed = live.map_or(0, |live| mbps_to_bps(live.download_speed.mbps));
        let upload_speed = live.map_or(0, |live| mbps_to_bps(live.upload_speed.mbps));
        let downloaded = if stats.file_progress.is_empty() {
            self.download.downloaded
        } else {
            self.selected
                .iter()
                .map(|&(index, size)| {
                    stats
                        .file_progress
                        .get(index)
                        .map_or(0, |&have| have.min(size))
                })
                .fold(0, u64::saturating_add)
        };
        let uploaded = self.base.uploaded.saturating_add(stats.uploaded_bytes);
        DownloadProgress {
            id: self.download.id,
            status,
            downloaded,
            total_size: Some(self.selected_bytes),
            speed: if status == DownloadStatus::Seeding {
                upload_speed
            } else {
                download_speed
            },
            torrent: Some(TorrentStats {
                peers: live.map_or(0, |live| live.snapshot.peer_stats.live),
                uploaded,
                upload_speed,
                ratio: ratio(uploaded, self.selected_bytes),
                seed_elapsed_secs,
                metadata_wait_secs: 0,
            }),
        }
    }

    async fn send(&self, progress: DownloadProgress) {
        let _ = self.progress_tx.send(progress).await;
    }

    async fn until_cancelled<T>(&self, future: impl Future<Output = T>) -> Result<T> {
        tokio::select! {
            () = self.cancel.cancelled() => Err(ShioError::Cancelled),
            value = future => Ok(value),
        }
    }
}

fn mbps_to_bps(mbps: f64) -> u64 {
    (mbps * 1024.0 * 1024.0) as u64
}

fn ratio(uploaded: u64, selected_bytes: u64) -> f32 {
    if selected_bytes == 0 {
        return 0.0;
    }
    (uploaded as f64 / selected_bytes as f64) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_policy_stops_at_its_limits() {
        let both = SeedPolicy::RatioOrTime {
            ratio: 1.0,
            seconds: 60,
        };
        for (policy, ratio, elapsed, expected) in [
            (SeedPolicy::StopAtRatio { ratio: 1.0 }, 0.9, u64::MAX, false),
            (SeedPolicy::StopAtRatio { ratio: 1.0 }, 1.0, 0, true),
            (SeedPolicy::StopAtTime { seconds: 60 }, 99.0, 59, false),
            (SeedPolicy::StopAtTime { seconds: 60 }, 0.0, 60, true),
            (both, 0.5, 30, false),
            (both, 1.0, 0, true),
            (both, 0.0, 60, true),
            (SeedPolicy::SeedForever, 999.0, u64::MAX, false),
            (SeedPolicy::NeverSeed, 0.0, 0, true),
        ] {
            assert_eq!(policy.should_stop(ratio, elapsed), expected, "{policy:?}");
        }
    }

    #[test]
    fn seed_policy_rejects_invalid_ratios() {
        for ratio in [f32::NAN, f32::INFINITY, -0.1] {
            assert!(SeedPolicy::StopAtRatio { ratio }.validate().is_err());
            assert!(
                SeedPolicy::RatioOrTime { ratio, seconds: 0 }
                    .validate()
                    .is_err()
            );
        }
        assert!(SeedPolicy::StopAtRatio { ratio: 0.0 }.validate().is_ok());
    }

    const PAYLOAD: &[u8] = &[7; 40_000];

    async fn staged_torrent(root: &Path, name: &str) -> (Download, Arc<Session>) {
        let source = root.join("source");
        std::fs::create_dir_all(&source).expect("source dir");
        std::fs::write(source.join(name), PAYLOAD).expect("payload");
        let created = librqbit::create_torrent(
            &source.join(name),
            librqbit::CreateTorrentOptions {
                name: None,
                trackers: Vec::new(),
                piece_length: Some(16_384),
            },
            &librqbit::spawn_utils::BlockingSpawner::new(1),
        )
        .await
        .expect("create torrent");
        let bytes = created.as_bytes().expect("torrent bytes").to_vec();
        let download = metainfo::tests::torrent_download(bytes, root.join("downloads"));
        let staged = ensure_owned_staging(&download).await.expect("staging");
        std::fs::write(staged.join(name), PAYLOAD).expect("staged payload");
        let session = Session::new_with_opts(
            root.join("session"),
            librqbit::SessionOptions {
                dht: None,
                persistence: None,
                listen: None,
                disable_local_service_discovery: true,
                ..Default::default()
            },
        )
        .await
        .expect("session");
        (download, session)
    }

    async fn run_never_seed(
        download: &Download,
        session: &Arc<Session>,
    ) -> (Result<()>, Vec<DownloadProgress>) {
        let mut config = AppConfig::default();
        config.torrent.seed_policy = SeedPolicy::NeverSeed;
        let (_config_tx, config_rx) = watch::channel(config);
        let (progress_tx, mut progress_rx) = mpsc::channel(64);
        let deps = TorrentWorkerDeps {
            session: session.clone(),
            progress_tx,
            cancel: CancellationToken::new(),
            config: config_rx,
            slot: ActiveSlot::detached(),
            metadata: MetadataCommit::detached(),
        };
        let result =
            tokio::time::timeout(Duration::from_secs(30), run_torrent(download.clone(), deps))
                .await
                .expect("worker timeout");
        let mut progress = Vec::new();
        while let Ok(update) = progress_rx.try_recv() {
            progress.push(update);
        }
        (result, progress)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn never_seed_publishes_and_keeps_final_progress() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (download, session) = staged_torrent(temp.path(), "data.bin").await;

        let (result, progress) = run_never_seed(&download, &session).await;

        result.expect("worker");
        let last = progress.last().expect("progress");
        assert_eq!(last.downloaded, PAYLOAD.len() as u64);
        assert_eq!(last.total_size, Some(PAYLOAD.len() as u64));
        let published = temp.path().join("downloads/data.bin");
        assert_eq!(std::fs::read(published).expect("published"), PAYLOAD);
        assert!(!staging_root(&download).exists());
        let info_hash = download.torrent().expect("torrent").info_hash;
        assert!(session.get(torrent_id(info_hash)).is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn resuming_a_published_torrent_does_not_extract_again() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (mut download, session) = staged_torrent(temp.path(), "data.rar").await;
        download.torrent_mut().expect("torrent").auto_extract = true;

        let (first, _) = run_never_seed(&download, &session).await;
        let (resumed, _) = run_never_seed(&download, &session).await;

        assert!(
            first.is_err(),
            "the bogus archive must fail its first extraction"
        );
        resumed.expect("resume skips extraction of the published payload");
    }
}
