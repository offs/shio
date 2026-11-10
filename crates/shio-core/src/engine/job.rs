use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::{Notify, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::{Context, Internal, PROGRESS_FLUSH_INTERVAL, ProgressEvent, WORKER_PROGRESS_CAPACITY};
use crate::error::{Result, ShioError};
use crate::probe::HttpProbe;
use crate::resolve::{Resolved, Target};
use crate::torrent::TorrentManifest;
use crate::types::{Download, DownloadId, DownloadProgress, DownloadStatus};
use crate::worker::blocking;

const RESOLVE_TIMEOUT: Duration = Duration::from_mins(2);

// Counts workers that occupy a `max_concurrent` slot; dropping one wakes the loop.
pub(crate) struct ActiveSlot(Arc<Slots>);

#[derive(Default)]
pub(super) struct Slots {
    pub(super) active: AtomicUsize,
    pub(super) freed: Notify,
}

impl ActiveSlot {
    pub(super) fn new(slots: &Arc<Slots>) -> Self {
        slots.active.fetch_add(1, Ordering::Relaxed);
        Self(slots.clone())
    }

    #[cfg(test)]
    pub(crate) fn detached() -> Self {
        Self::new(&Arc::default())
    }
}

impl Drop for ActiveSlot {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::Relaxed);
        self.0.freed.notify_one();
    }
}

pub(crate) struct MetadataCommit {
    id: DownloadId,
    loop_tx: mpsc::UnboundedSender<Internal>,
}

impl MetadataCommit {
    pub(crate) async fn commit(
        &self,
        manifest: TorrentManifest,
        metadata_bytes: Vec<u8>,
    ) -> Result<()> {
        let (reply, result) = oneshot::channel();
        self.loop_tx
            .send(Internal::TorrentMetadata {
                id: self.id,
                manifest,
                metadata_bytes,
                reply,
            })
            .map_err(|_| ShioError::Cancelled)?;
        result.await.map_err(|_| ShioError::Cancelled)?
    }

    #[cfg(test)]
    pub(crate) fn detached() -> Self {
        let (loop_tx, mut loop_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(message) = loop_rx.recv().await {
                if let Internal::TorrentMetadata { reply, .. } = message {
                    let _ = reply.send(Ok(()));
                }
            }
        });
        Self {
            id: DownloadId::new(),
            loop_tx,
        }
    }
}

/// Progress moves a row between active statuses in memory only; a status that recovery must
/// see, such as `Extracting` before any staging exists, is committed through the engine.
pub(crate) struct StatusCommit {
    id: DownloadId,
    loop_tx: mpsc::UnboundedSender<Internal>,
}

impl StatusCommit {
    pub(crate) async fn commit(&self, status: DownloadStatus) -> Result<()> {
        let (reply, result) = oneshot::channel();
        self.loop_tx
            .send(Internal::CommitStatus {
                id: self.id,
                status,
                reply,
            })
            .map_err(|_| ShioError::Cancelled)?;
        result.await.map_err(|_| ShioError::Cancelled)?
    }

    #[cfg(test)]
    pub(crate) fn detached() -> Self {
        Self {
            id: DownloadId::new(),
            loop_tx: mpsc::unbounded_channel().0,
        }
    }
}

pub(super) async fn run_http(
    ctx: Context,
    http: reqwest::Client,
    mut download: Download,
    may_rename: bool,
    cancel: CancellationToken,
) -> Result<()> {
    let id = download.id;
    let (progress_tx, progress_rx) = mpsc::channel(WORKER_PROGRESS_CAPACITY);
    let work = async {
        let (url, headers) = download
            .http()
            .map(|http| (http.url.clone(), http.headers.clone()))
            .ok_or_else(|| ShioError::Other("expected an HTTP download".into()))?;
        let max_retries = download.max_retries;
        let published = {
            let download = download.clone();
            blocking(move || crate::worker::recover_published(&download)).await?
        };
        if let Some(size) = published {
            let job = crate::worker::HttpJob {
                config: ctx.shared.config.subscribe(),
                client: http,
                db: ctx.db.clone(),
                progress_tx,
                status: StatusCommit {
                    id,
                    loop_tx: ctx.loop_tx.clone(),
                },
                cancel,
                limiter: ctx.shared.limiter.clone(),
                host: ctx.shared.hosts.get(&url),
            };
            return crate::worker::after_publish(&download, size, &job).await;
        }
        let direct = crate::probe::retrying(max_retries, &cancel, || {
            crate::probe::probe(&http, &url, &headers)
        })
        .await;
        let (probe, resolved) = match direct {
            Ok(probe) => (probe, None),
            Err(ShioError::NotADirectFile { .. } | ShioError::BrowserCheck) => {
                let (probe, page) = probe_page(&http, &url, &headers, max_retries, &cancel).await?;
                (probe, Some(page))
            },
            Err(error) => return Err(error),
        };
        if may_rename && let Some(name) = probe.server_filename(&download, resolved.is_some()) {
            let (reply, claimed) = oneshot::channel();
            ctx.loop_tx
                .send(Internal::ClaimFilename {
                    id,
                    filename: name.to_owned(),
                    reply,
                })
                .map_err(|_| ShioError::Cancelled)?;
            download.filename = claimed.await.map_err(|_| ShioError::Cancelled)??;
        }
        let host = ctx
            .shared
            .hosts
            .get(resolved.as_ref().map_or(&url, |page| &page.url));
        if let Some(page) = resolved
            && let Some(state) = download.http_mut()
        {
            state.url = page.url;
            state.headers = page.headers;
        }
        let job = crate::worker::HttpJob {
            config: ctx.shared.config.subscribe(),
            client: http,
            db: ctx.db.clone(),
            progress_tx,
            status: StatusCommit {
                id,
                loop_tx: ctx.loop_tx.clone(),
            },
            cancel,
            limiter: ctx.shared.limiter.clone(),
            host,
        };
        crate::worker::run(download, probe, job).await
    };
    let (result, ()) = tokio::join!(work, track_progress(&ctx, id, progress_rx));
    result
}

async fn probe_page(
    http: &reqwest::Client,
    url: &str,
    headers: &[(String, String)],
    max_retries: u32,
    cancel: &CancellationToken,
) -> Result<(HttpProbe, Resolved)> {
    let resolve = || async {
        tokio::time::timeout(RESOLVE_TIMEOUT, crate::resolve::resolve(http, url))
            .await
            .map_err(|_| ShioError::Other("download page timed out".into()))?
    };
    let page = match crate::probe::retrying(max_retries, cancel, resolve).await? {
        Target::File(page) => page,
        Target::Folder(_) => {
            return Err(ShioError::Other(
                "folder link; add it again to pick files".into(),
            ));
        },
    };
    let mut merged: Vec<(String, String)> = headers
        .iter()
        .filter(|(name, _)| {
            !page
                .headers
                .iter()
                .any(|(page_name, _)| page_name.eq_ignore_ascii_case(name))
        })
        .cloned()
        .collect();
    merged.extend(page.headers);
    let probe = crate::probe::retrying(max_retries, cancel, || {
        crate::probe::probe(http, &page.url, &merged)
    })
    .await?;
    Ok((
        probe,
        Resolved {
            url: page.url,
            headers: merged,
        },
    ))
}

pub(super) async fn run_torrent(
    ctx: Context,
    download: Download,
    cancel: CancellationToken,
    slot: ActiveSlot,
) -> Result<()> {
    let id = download.id;
    let session = tokio::select! {
        session = ctx.shared.session() => session?,
        () = cancel.cancelled() => return Err(ShioError::Cancelled),
    };
    let (progress_tx, progress_rx) = mpsc::channel(WORKER_PROGRESS_CAPACITY);
    let deps = crate::torrent::TorrentWorkerDeps {
        session,
        progress_tx,
        cancel,
        config: ctx.shared.config.subscribe(),
        slot,
        metadata: MetadataCommit {
            id,
            loop_tx: ctx.loop_tx.clone(),
        },
        status: StatusCommit {
            id,
            loop_tx: ctx.loop_tx.clone(),
        },
    };
    let (result, ()) = tokio::join!(
        crate::torrent::run_torrent(download, deps),
        track_progress(&ctx, id, progress_rx)
    );
    result
}

// Workers may move a row between active statuses only.
async fn track_progress(
    ctx: &Context,
    id: DownloadId,
    mut progress_rx: mpsc::Receiver<DownloadProgress>,
) {
    let mut flushed: Option<Instant> = None;
    while let Some(mut progress) = progress_rx.recv().await {
        let status_changed = {
            let mut rows = ctx.shared.rows.write();
            let Some(row) = rows.downloads.get_mut(&id) else {
                continue;
            };
            row.downloaded = progress.downloaded;
            row.total_size = progress.total_size.or(row.total_size);
            row.speed = progress.speed;
            if let (Some(stats), Some(torrent)) = (progress.torrent, row.torrent_mut()) {
                torrent.stats = stats;
            }
            let changed = row.status != progress.status
                && row.status.is_active()
                && progress.status.is_active();
            if changed {
                row.status = progress.status;
            }
            progress.status = row.status;
            progress.total_size = row.total_size;
            let _ = ctx.events.send(ProgressEvent::Update(progress));
            changed
        };
        if status_changed {
            ctx.shared.refresh_speed_limits();
        }
        if flushed.is_none_or(|at| at.elapsed() >= PROGRESS_FLUSH_INTERVAL) {
            flushed = Some(Instant::now());
            let db = ctx.db.clone();
            let flush = blocking(move || {
                db.update_progress(
                    id,
                    progress.downloaded,
                    progress.total_size,
                    progress.torrent.as_ref(),
                )
            });
            if let Err(error) = flush.await {
                tracing::warn!(download = %id, "progress flush failed: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::engine::DownloadEngine;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    async fn server(response: &'static [u8]) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut request = vec![0_u8; 4096];
                let _ = socket.read(&mut request).await;
                let _ = socket.write_all(response).await;
            }
        });
        format!("http://{address}/file.bin")
    }

    #[tokio::test]
    async fn a_published_payload_completes_without_probing_again() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            download_dir: dir.path().to_owned(),
            ..AppConfig::default()
        };
        let (engine, _commands, _events) = DownloadEngine::new(config, dir.path()).expect("engine");
        let ok =
            server(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello").await;
        let mut download = Download::new(ok, dir.path().to_owned());
        engine.ctx.db.insert_download(&download).expect("row");
        let run = |download: Download| {
            run_http(
                engine.ctx.clone(),
                engine.http.clone(),
                download,
                false,
                CancellationToken::new(),
            )
        };
        run(download.clone()).await.expect("first run publishes");

        let expired = server(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n").await;
        download.http_mut().expect("http").url = expired;
        run(download.clone())
            .await
            .expect("published payload needs no probe");

        assert_eq!(std::fs::read(download.file_path()).expect("file"), b"hello");
    }
}
