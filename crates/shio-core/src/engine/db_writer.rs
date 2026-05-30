use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::db::Database;
use crate::types::{DownloadId, DownloadStatus, TorrentFile};

pub(super) enum DbWrite {
    Progress {
        id: DownloadId,
        downloaded: u64,
        total_size: Option<u64>,
    },
    Final {
        id: DownloadId,
        status: DownloadStatus,
        downloaded: u64,
        total_size: Option<u64>,
        torrent_runtime: Option<TorrentRuntimeWrite>,
    },
    Metadata {
        id: DownloadId,
        filename: String,
        save_path: PathBuf,
    },
    TorrentMetadata {
        id: DownloadId,
        is_private: bool,
        files: Vec<TorrentFile>,
        trackers: Vec<String>,
    },
    TorrentRuntime {
        id: DownloadId,
        uploaded: u64,
        ratio: f32,
        seed_elapsed_secs: u64,
    },
}

pub(super) struct TorrentRuntimeWrite {
    pub(super) uploaded: u64,
    pub(super) ratio: f32,
    pub(super) seed_elapsed_secs: u64,
}

pub(super) fn spawn_db_writer(
    db: Arc<Database>,
    mut rx: mpsc::Receiver<DbWrite>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(write) = rx.recv().await {
            let db = db.clone();
            if let Err(e) = tokio::task::spawn_blocking(move || apply_db_write(&db, write)).await {
                tracing::error!("db write task failed: {e}");
            }
        }
    })
}

fn apply_db_write(db: &Database, write: DbWrite) {
    match write {
        DbWrite::Progress {
            id,
            downloaded,
            total_size,
        } => {
            log_db(
                "update_progress",
                id,
                db.update_progress(id, downloaded, total_size),
            );
        },
        DbWrite::Final {
            id,
            status,
            downloaded,
            total_size,
            torrent_runtime,
        } => {
            log_db_critical(
                "update_final",
                id,
                db.update_final(id, status, downloaded, total_size),
            );
            if let Some(runtime) = torrent_runtime {
                log_db_critical(
                    "update_torrent_runtime final",
                    id,
                    db.update_torrent_runtime(
                        id,
                        runtime.uploaded,
                        runtime.ratio,
                        runtime.seed_elapsed_secs,
                    ),
                );
            }
        },
        DbWrite::Metadata {
            id,
            filename,
            save_path,
        } => {
            log_db_critical(
                "update_metadata",
                id,
                db.update_metadata(id, &filename, &save_path),
            );
        },
        DbWrite::TorrentMetadata {
            id,
            is_private,
            files,
            trackers,
        } => {
            log_db_critical(
                "update_torrent_metadata",
                id,
                db.update_torrent_metadata(id, is_private, &files, &trackers),
            );
        },
        DbWrite::TorrentRuntime {
            id,
            uploaded,
            ratio,
            seed_elapsed_secs,
        } => {
            log_db(
                "update_torrent_runtime",
                id,
                db.update_torrent_runtime(id, uploaded, ratio, seed_elapsed_secs),
            );
        },
    }
}

pub(super) fn log_db(op: &str, id: DownloadId, result: crate::Result<()>) {
    if let Err(e) = result {
        tracing::warn!(download = %id, op, "db write failed: {e}");
    }
}

pub(super) fn log_db_critical(op: &str, id: DownloadId, result: crate::Result<()>) {
    if let Err(e) = result {
        tracing::error!(download = %id, op, "critical db write failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Download;
    use std::time::Duration;

    #[tokio::test]
    async fn writer_flushes_queued_final_write_before_exit() {
        let db = Arc::new(Database::in_memory().unwrap());
        let mut download = Download::new(
            "https://example.com/file.bin".to_string(),
            std::path::PathBuf::from("/tmp"),
        );
        download.filename = "file.bin".to_string();
        let id = download.id;
        db.insert_download(&download).unwrap();
        let (tx, rx) = mpsc::channel(4);
        let handle = spawn_db_writer(db.clone(), rx);

        tx.send(DbWrite::Final {
            id,
            status: DownloadStatus::Completed,
            downloaded: 20,
            total_size: Some(20),
            torrent_runtime: None,
        })
        .await
        .unwrap();
        drop(tx);

        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .unwrap()
            .unwrap();
        let stored = db.get(id).unwrap().unwrap();
        assert_eq!(stored.status, DownloadStatus::Completed);
        assert_eq!(stored.downloaded, 20);
        assert_eq!(stored.total_size, Some(20));
    }
}
