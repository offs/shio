use crate::chunk::{ChunkDownloader, ChunkPlan, ChunkProgress};
use crate::config::AppConfig;
use crate::db::Database;
use crate::error::{Result, ShioError};
use crate::probe::{ServerProbe, probe_server};
use crate::types::{
    ChunkInfo, ChunkStatus, Download, DownloadProgress, DownloadStatus, HttpState, ProgressDetail,
    ProxyProtocol,
};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const MAX_CHUNK_RETRIES: u32 = 3;
const PROGRESS_INTERVAL_MS: u128 = 100;
const CHUNK_DB_FLUSH_MS: u128 = 500;
const MIN_CHUNK_BYTES: u64 = 1024 * 1024;
const RECV_TIMEOUT_MS: u64 = 50;
const SPEED_WINDOW_MS: u128 = 3000;
const ETA_GRACE_MS: u128 = 1000;
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

fn http_state(d: &Download) -> Result<&HttpState> {
    d.kind
        .as_http()
        .ok_or_else(|| ShioError::Other("worker: expected HTTP kind".into()))
}

pub(crate) fn build_client(config: &AppConfig) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .redirect(reqwest::redirect::Policy::limited(10))
        .connect_timeout(Duration::from_secs(30))
        .pool_idle_timeout(Duration::from_secs(90))
        .cookie_store(true);

    if let Some(proxy) = config.proxy.as_ref() {
        let url = match proxy.protocol {
            ProxyProtocol::Http => format!("http://{}:{}", proxy.host, proxy.port),
            ProxyProtocol::Socks5 => format!("socks5://{}:{}", proxy.host, proxy.port),
        };
        let mut p = reqwest::Proxy::all(&url)?;
        if let (Some(user), Some(pass)) = (&proxy.username, &proxy.password) {
            p = p.basic_auth(user, pass);
        }
        builder = builder.proxy(p);
    }

    Ok(builder.build()?)
}

fn plan_download(probe: &ServerProbe, config: &AppConfig) -> ChunkPlan {
    let Some(total) = probe.content_length else {
        return ChunkPlan::single_stream();
    };
    if total == 0 || !probe.accept_ranges {
        return ChunkPlan::single_stream();
    }
    let segments = config.default_segments.clamp(1, 32);
    let segments = if total > u64::from(segments) * MIN_CHUNK_BYTES {
        segments
    } else {
        1
    };
    ChunkPlan::plan_chunks(total, segments)
}

async fn ensure_output_file(file_path: &std::path::Path, total: u64) -> Result<()> {
    let needs_allocation = match tokio::fs::metadata(file_path).await {
        Ok(meta) => meta.len() != total,
        Err(_) => true,
    };
    if needs_allocation {
        let f = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(file_path)
            .await?;
        f.set_len(total).await?;
    }
    Ok(())
}

enum ChunkJoinOutcome {
    AllOk,
    Cancelled,
    Failed(String),
}

async fn join_chunk_handles(
    handles: Vec<tokio::task::JoinHandle<Result<u32>>>,
) -> ChunkJoinOutcome {
    let mut error: Option<String> = None;
    for handle in handles {
        match handle.await {
            Ok(Ok(_)) => {},
            Ok(Err(ShioError::Cancelled)) => return ChunkJoinOutcome::Cancelled,
            Ok(Err(e)) => {
                tracing::warn!("chunk error: {e}");
                error.get_or_insert_with(|| e.to_string());
            },
            Err(e) => {
                tracing::warn!("chunk task join error: {e}");
                error.get_or_insert_with(|| format!("task join error: {e}"));
            },
        }
    }
    match error {
        None => ChunkJoinOutcome::AllOk,
        Some(e) => ChunkJoinOutcome::Failed(e),
    }
}

async fn aggregate_progress(
    id: crate::types::DownloadId,
    total_size: Option<u64>,
    mut chunk_states: Vec<ChunkInfo>,
    mut chunk_rx: mpsc::Receiver<ChunkProgress>,
    progress_tx: &mpsc::Sender<DownloadProgress>,
    db: &Arc<Database>,
) -> Vec<ChunkInfo> {
    let start = Instant::now();
    let mut last_emit = start;
    let mut last_db_flush = start;
    let start_bytes: u64 = chunk_states.iter().map(|c| c.downloaded).sum();
    let mut samples: std::collections::VecDeque<(Instant, u64)> = std::collections::VecDeque::new();
    samples.push_back((start, start_bytes));

    loop {
        match tokio::time::timeout(Duration::from_millis(RECV_TIMEOUT_MS), chunk_rx.recv()).await {
            Ok(Some(p)) => {
                let idx = p.chunk_index as usize;
                if let Some(chunk) = chunk_states.get_mut(idx) {
                    chunk.downloaded = p.downloaded;
                    chunk.status = ChunkStatus::Downloading;
                } else {
                    tracing::warn!(
                        download = %id,
                        chunk_index = p.chunk_index,
                        chunk_count = chunk_states.len(),
                        "out-of-bounds chunk progress dropped"
                    );
                }
            },
            Ok(None) => break,
            Err(_) => {},
        }

        if last_emit.elapsed().as_millis() >= PROGRESS_INTERVAL_MS {
            let now = Instant::now();
            let downloaded: u64 = chunk_states.iter().map(|c| c.downloaded).sum();
            samples.push_back((now, downloaded));
            let window = Duration::from_millis(SPEED_WINDOW_MS as u64);
            while samples
                .front()
                .is_some_and(|(t, _)| now.duration_since(*t) > window)
            {
                samples.pop_front();
            }

            let speed = samples.front().map_or(0, |(t_old, b_old)| {
                let dt_ms = now.duration_since(*t_old).as_millis() as u64;
                if dt_ms > 0 {
                    downloaded.saturating_sub(*b_old).saturating_mul(1000) / dt_ms
                } else {
                    0
                }
            });
            let avg_speed = if start.elapsed().as_millis() >= ETA_GRACE_MS {
                speed
            } else {
                0
            };

            let _ = progress_tx.try_send(DownloadProgress {
                id,
                downloaded,
                total_size,
                speed,
                avg_speed,
                status: DownloadStatus::Downloading,
                detail: ProgressDetail::Http {
                    chunks: chunk_states.clone(),
                },
                filename: None,
                torrent_snapshot: None,
            });
            last_emit = now;
        }

        if last_db_flush.elapsed().as_millis() >= CHUNK_DB_FLUSH_MS {
            let snapshot: Vec<(u32, u64, ChunkStatus)> = chunk_states
                .iter()
                .map(|c| (c.index, c.downloaded, c.status))
                .collect();
            let db = db.clone();
            tokio::task::spawn_blocking(move || {
                for (i, d, s) in snapshot {
                    if let Err(e) = db.update_chunk(id, i, d, s) {
                        tracing::warn!(download = %id, chunk = i, "chunk flush failed: {e}");
                    }
                }
            });
            last_db_flush = Instant::now();
        }
    }
    chunk_states
}

async fn run_chunk_with_retry(
    mut downloader: ChunkDownloader,
    tx: mpsc::Sender<ChunkProgress>,
    cancel: CancellationToken,
) -> Result<u32> {
    let mut retries = 0_u32;
    loop {
        match downloader.download(tx.clone()).await {
            Ok(()) => return Ok(downloader.chunk.index),
            Err(ShioError::Cancelled) => return Err(ShioError::Cancelled),
            Err(e) => {
                retries += 1;
                if retries > MAX_CHUNK_RETRIES {
                    return Err(e);
                }
                let delay =
                    Duration::from_secs(1_u64 << (retries - 1)).min(Duration::from_secs(60));
                tracing::warn!(
                    "chunk {} retry {}/{} in {:?}: {e}",
                    downloader.chunk.index,
                    retries,
                    MAX_CHUNK_RETRIES,
                    delay
                );
                tokio::select! {
                    () = cancel.cancelled() => return Err(ShioError::Cancelled),
                    () = tokio::time::sleep(delay) => {},
                }
            },
        }
    }
}

pub(crate) const fn status_for_extract_result(result: &Result<()>) -> DownloadStatus {
    match result {
        Ok(()) => DownloadStatus::Completed,
        Err(ShioError::PasswordRequired) => DownloadStatus::PasswordRequired,
        Err(_) => DownloadStatus::ExtractError,
    }
}

async fn emit_status(
    progress_tx: &mpsc::Sender<DownloadProgress>,
    id: crate::types::DownloadId,
    total: Option<u64>,
    chunks: &[ChunkInfo],
    status: DownloadStatus,
) {
    let downloaded: u64 = chunks.iter().map(|c| c.downloaded).sum();
    let _ = progress_tx
        .send(DownloadProgress {
            id,
            downloaded,
            total_size: total,
            speed: 0,
            avg_speed: 0,
            status,
            detail: ProgressDetail::Http {
                chunks: chunks.to_vec(),
            },
            filename: None,
            torrent_snapshot: None,
        })
        .await;
}

#[derive(Debug)]
pub(crate) struct DownloadWorker;

impl DownloadWorker {
    pub(crate) async fn run(
        mut download: Download,
        config: &AppConfig,
        client: reqwest::Client,
        db: Arc<Database>,
        progress_tx: mpsc::Sender<DownloadProgress>,
        cancel: CancellationToken,
    ) -> Result<DownloadStatus> {
        tracing::debug!("worker started for {}", download.filename);

        let http = http_state(&download)?;
        let network_url = url_without_fragment(&http.url);
        let headers = http.headers.clone();
        let probe = probe_server(&client, &network_url, &headers).await?;
        tracing::debug!(
            "probe: url={} content_type={:?} content_length={:?} accept_ranges={} attachment={}",
            probe.final_url,
            probe.content_type,
            probe.content_length,
            probe.accept_ranges,
            probe.has_attachment
        );

        if download.filename == "download" && probe.response_filename != "download" {
            download.filename.clone_from(&probe.response_filename);
        }
        download.total_size = probe.content_length;
        if let Some(http) = download.kind.as_http_mut() {
            http.url.clone_from(&probe.final_url);
        }

        let file_path = download.file_path();
        if let Some(parent) = file_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        if let Some(total) = probe.content_length {
            ensure_output_file(&file_path, total).await?;
        }

        let saved_chunks = db.chunks(download.id)?;
        let planned = plan_download(&probe, config);

        let chunk_states = if chunks_are_resumable(&saved_chunks, &planned) {
            saved_chunks
        } else {
            if let Err(e) = db.delete_chunks(download.id) {
                tracing::warn!(download = %download.id, "delete_chunks failed: {e}");
            }
            planned.chunks.clone()
        };

        let mut chunk_states = if chunk_states.is_empty() {
            planned.chunks
        } else {
            chunk_states
        };

        if let Err(e) = db.insert_chunks(download.id, &chunk_states) {
            tracing::warn!(download = %download.id, "insert_chunks failed: {e}");
        }

        let num_chunks = chunk_states.len();
        let per_chunk_speed_limit = config.speed_limit.map(|l| l / num_chunks as u64);

        let (chunk_tx, chunk_rx) = mpsc::channel::<ChunkProgress>(num_chunks * 4 + 16);
        let mut handles = Vec::with_capacity(num_chunks);

        for chunk in chunk_states.clone() {
            let child_cancel = cancel.child_token();
            let downloader = ChunkDownloader {
                client: client.clone(),
                url: probe.final_url.clone(),
                chunk,
                file_path: file_path.clone(),
                headers: headers.clone(),
                cancel: child_cancel.clone(),
                speed_limit: per_chunk_speed_limit,
                require_range: num_chunks > 1,
            };
            let tx = chunk_tx.clone();
            handles.push(tokio::spawn(run_chunk_with_retry(
                downloader,
                tx,
                child_cancel,
            )));
        }
        drop(chunk_tx);

        chunk_states = aggregate_progress(
            download.id,
            download.total_size,
            chunk_states,
            chunk_rx,
            &progress_tx,
            &db,
        )
        .await;

        match join_chunk_handles(handles).await {
            ChunkJoinOutcome::Cancelled => Ok(DownloadStatus::Cancelled),
            ChunkJoinOutcome::AllOk => {
                for chunk in &mut chunk_states {
                    chunk.status = ChunkStatus::Completed;
                }
                let downloaded: u64 = chunk_states.iter().map(|c| c.downloaded).sum();
                if let Some(expected) = download.total_size {
                    if downloaded != expected {
                        emit_status(
                            &progress_tx,
                            download.id,
                            download.total_size,
                            &chunk_states,
                            DownloadStatus::Error,
                        )
                        .await;
                        return Err(ShioError::SizeMismatch {
                            expected,
                            actual: downloaded,
                        });
                    }
                }

                if http_state(&download)?.auto_extract
                    && crate::filename::has_unsupported_archive_extension(&download.filename)
                {
                    let message = format!("unsupported archive format: {}", download.filename);
                    emit_status(
                        &progress_tx,
                        download.id,
                        download.total_size,
                        &chunk_states,
                        DownloadStatus::ExtractError,
                    )
                    .await;
                    return Err(ShioError::Extract(message));
                }

                if http_state(&download)?.auto_extract && extraction_ready(&download).await {
                    emit_status(
                        &progress_tx,
                        download.id,
                        download.total_size,
                        &chunk_states,
                        DownloadStatus::Extracting,
                    )
                    .await;
                    let extract_result = run_extract(&download, config, None).await;
                    let status = status_for_extract_result(&extract_result);
                    emit_status(
                        &progress_tx,
                        download.id,
                        download.total_size,
                        &chunk_states,
                        status,
                    )
                    .await;
                    return extract_result.map(|()| DownloadStatus::Completed);
                }
                emit_status(
                    &progress_tx,
                    download.id,
                    download.total_size,
                    &chunk_states,
                    DownloadStatus::Completed,
                )
                .await;
                Ok(DownloadStatus::Completed)
            },
            ChunkJoinOutcome::Failed(msg) => {
                emit_status(
                    &progress_tx,
                    download.id,
                    download.total_size,
                    &chunk_states,
                    DownloadStatus::Error,
                )
                .await;
                Err(ShioError::Other(msg))
            },
        }
    }
}

fn chunks_are_resumable(saved: &[ChunkInfo], planned: &ChunkPlan) -> bool {
    if saved.len() != planned.chunks.len() {
        return false;
    }
    saved
        .iter()
        .zip(&planned.chunks)
        .all(|(a, b)| a.start == b.start && a.end == b.end)
}

fn extract_target(download: &Download) -> Option<std::path::PathBuf> {
    if download.kind.is_http() {
        return Some(download.file_path());
    }

    let torrent = download.torrent()?;
    let selected: Vec<_> = torrent.files.iter().filter(|file| file.selected).collect();
    let files = if selected.is_empty() {
        torrent.files.iter().collect::<Vec<_>>()
    } else {
        selected
    };

    let mut plans = std::collections::BTreeSet::new();
    for file in files {
        let path = download.save_path.join(&file.path);
        let Some(name) = path.file_name().and_then(std::ffi::OsStr::to_str) else {
            continue;
        };
        if !crate::is_archive_filename(name) {
            continue;
        }
        let Some(plan) = crate::extract::plan(&path) else {
            continue;
        };
        plans.insert(plan.first_volume);
    }

    if plans.len() == 1 {
        return plans.into_iter().next();
    }

    None
}

pub(crate) async fn extraction_ready(download: &Download) -> bool {
    let Some(target) = extract_target(download) else {
        return false;
    };
    let Some(plan) = crate::extract::plan(&target) else {
        return false;
    };
    match tokio::task::spawn_blocking(move || crate::extract::is_complete(&plan)).await {
        Ok(ready) => ready,
        Err(e) => {
            tracing::warn!(download = %download.id, "extraction_ready panicked: {e}");
            false
        },
    }
}

pub(crate) async fn run_extract(
    download: &Download,
    config: &AppConfig,
    password: Option<String>,
) -> Result<()> {
    let Some(target) = extract_target(download) else {
        return Err(ShioError::Extract("no extractable archive found".into()));
    };
    let Some(plan) = crate::extract::plan(&target) else {
        return Err(ShioError::Extract("archive set is incomplete".into()));
    };
    let parent = target
        .parent()
        .map_or_else(|| download.file_dir(), std::path::Path::to_path_buf);
    let dest = if config.extract_to_subfolder {
        parent.join(&plan.base_name)
    } else {
        parent
    };
    let archive = plan.first_volume.clone();
    let members = plan.members.clone();
    let dest_clone = dest.clone();
    let archive_clone = archive.clone();
    let result = tokio::task::spawn_blocking(move || {
        crate::extract::extract(&archive_clone, &dest_clone, password.as_deref())
    })
    .await;
    match result {
        Ok(Ok(_)) => {
            tracing::info!("extracted {} -> {}", archive.display(), dest.display());
            if config.delete_archive_after_extract {
                for member in &members {
                    if let Err(e) = tokio::fs::remove_file(member).await {
                        tracing::warn!("failed to delete {}: {e}", member.display());
                    }
                }
            }
            Ok(())
        },
        Ok(Err(e)) => {
            tracing::warn!("extract failed for {}: {e}", archive.display());
            Err(e)
        },
        Err(e) => {
            tracing::warn!("extract task panicked: {e}");
            Err(ShioError::Extract(format!("extract task panicked: {e}")))
        },
    }
}

pub(crate) fn url_without_fragment(url: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(url) else {
        return url.to_string();
    };
    parsed.set_fragment(None);
    parsed.to_string()
}

#[cfg(test)]
mod tests {
    use super::{run_extract, status_for_extract_result, url_without_fragment};
    use crate::{AppConfig, Download, DownloadStatus};
    use std::path::PathBuf;

    #[test]
    fn url_without_fragment_preserves_query_for_network_request() {
        let url = "https://fuckingfast.co/abc123?dl=1#release.part01.rar";

        let stripped = url_without_fragment(url);

        assert_eq!(stripped, "https://fuckingfast.co/abc123?dl=1");
    }

    #[tokio::test]
    async fn retry_extract_without_archive_target_is_extract_error() {
        let download = Download::new(
            "https://example.com/file.bin".to_string(),
            PathBuf::from("/tmp"),
        );

        let result = run_extract(&download, &AppConfig::default(), None).await;

        assert!(result.is_err());
        assert_eq!(
            status_for_extract_result(&result),
            DownloadStatus::ExtractError
        );
    }
}
