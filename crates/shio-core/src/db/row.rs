use std::path::PathBuf;

use uuid::Uuid;

use super::codec::{
    decode_bool_i32, decode_bool_i64, decode_optional_u64, decode_ratio, decode_required_json,
    decode_u8, decode_u32, decode_u64, parse_optional_ts, parse_required_ts, require_f64,
    require_i64,
};
use crate::error::{Result, ShioError};
use crate::path::{validate_leaf_filename, validate_relative_path};
use crate::types::{
    ChunkInfo, ChunkStatus, Download, DownloadId, DownloadKind, DownloadStatus, HttpState,
    TorrentSource, TorrentState,
};

#[cfg(test)]
pub(super) const DOWNLOAD_SELECT: &str = "SELECT
    d.id, d.filename, d.save_path, d.total_size, d.downloaded, d.status,
    d.priority, d.error_msg, d.created_at, d.started_at, d.completed_at,
    d.retry_count, d.max_retries, d.pinned, d.kind,
    h.url, h.headers, h.segments, h.subfolder, h.auto_extract,
    t.source_kind, t.source_data, t.info_hash, t.auto_extract, t.is_private,
    t.files_json, t.trackers_json, t.uploaded, t.ratio, t.seed_elapsed_secs, t.metadata_bytes
    FROM downloads d
    LEFT JOIN http_state h ON h.download_id = d.id
    LEFT JOIN torrent_state t ON t.download_id = d.id
    WHERE d.id = ?1";

pub(super) const DOWNLOAD_SELECT_ALL: &str = "SELECT
    d.id, d.filename, d.save_path, d.total_size, d.downloaded, d.status,
    d.priority, d.error_msg, d.created_at, d.started_at, d.completed_at,
    d.retry_count, d.max_retries, d.pinned, d.kind,
    h.url, h.headers, h.segments, h.subfolder, h.auto_extract,
    t.source_kind, t.source_data, t.info_hash, t.auto_extract, t.is_private,
    t.files_json, t.trackers_json, t.uploaded, t.ratio, t.seed_elapsed_secs, t.metadata_bytes
    FROM downloads d
    LEFT JOIN http_state h ON h.download_id = d.id
    LEFT JOIN torrent_state t ON t.download_id = d.id";

pub(super) fn row_to_download(row: &rusqlite::Row<'_>) -> Result<Download> {
    let id_str: String = row.get(0)?;
    let id = Uuid::parse_str(&id_str)
        .map_err(|e| ShioError::Other(format!("invalid uuid {id_str}: {e}")))?;

    let filename: String = row.get(1)?;
    validate_leaf_filename(&filename)?;
    let save_path: String = row.get(2)?;
    let total_size: Option<i64> = row.get(3)?;
    let downloaded: i64 = row.get(4)?;
    let status_str: String = row.get(5)?;
    let priority: i32 = row.get(6)?;
    let error_message: Option<String> = row.get(7)?;
    let created_at: String = row.get(8)?;
    let started_at: Option<String> = row.get(9)?;
    let completed_at: Option<String> = row.get(10)?;
    let retry_count: i32 = row.get(11)?;
    let max_retries: i32 = row.get(12)?;
    let pinned: i32 = row.get(13)?;
    let kind_str: String = row.get(14)?;

    let kind = match kind_str.as_str() {
        "http" => {
            let url: Option<String> = row.get(15)?;
            let url =
                url.ok_or_else(|| ShioError::Other("http row missing url in http_state".into()))?;
            let headers_json: Option<String> = row.get(16)?;
            let headers = decode_required_json("http_state.headers", headers_json.as_deref())?;
            let segments: i32 = row.get(17)?;
            let subfolder: Option<String> = row.get(18)?;
            if let Some(subfolder) = subfolder.as_deref() {
                validate_leaf_filename(subfolder)?;
            }
            let auto_extract: i32 = row.get(19)?;
            DownloadKind::Http(HttpState {
                url,
                headers,
                segments: decode_u8("http_state.segments", segments)?,
                subfolder,
                auto_extract: decode_bool_i32("http_state.auto_extract", auto_extract)?,
            })
        },
        "torrent" => {
            let source_kind: Option<String> = row.get(20)?;
            let source_kind = source_kind
                .ok_or_else(|| ShioError::Other("torrent row missing source_kind".into()))?;
            let source_data: Option<Vec<u8>> = row.get(21)?;
            let source_data = source_data
                .ok_or_else(|| ShioError::Other("torrent row missing source_data".into()))?;
            let info_hash_blob: Option<Vec<u8>> = row.get(22)?;
            let info_hash_blob = info_hash_blob
                .ok_or_else(|| ShioError::Other("torrent row missing info_hash".into()))?;
            let info_hash: [u8; 20] = info_hash_blob
                .as_slice()
                .try_into()
                .map_err(|_| ShioError::Other("torrent info_hash not 20 bytes".into()))?;
            let auto_extract: Option<i64> = row.get(23)?;
            let is_private: Option<i64> = row.get(24)?;
            let files_json: Option<String> = row.get(25)?;
            let trackers_json: Option<String> = row.get(26)?;
            let uploaded: Option<i64> = row.get(27)?;
            let ratio: Option<f64> = row.get(28)?;
            let seed_elapsed_secs: Option<i64> = row.get(29)?;
            let metadata_bytes: Option<Vec<u8>> = row.get(30)?;

            let source = match source_kind.as_str() {
                "magnet" => {
                    let s = String::from_utf8(source_data)
                        .map_err(|e| ShioError::Other(format!("invalid magnet utf8: {e}")))?;
                    TorrentSource::Magnet(s)
                },
                "file" => TorrentSource::File(source_data),
                other => {
                    return Err(ShioError::Other(format!(
                        "unknown torrent source kind: {other}"
                    )));
                },
            };

            let files: Vec<crate::types::TorrentFile> =
                decode_required_json("torrent_state.files_json", files_json.as_deref())?;
            for file in &files {
                validate_relative_path(&file.path)?;
            }
            let trackers =
                decode_required_json("torrent_state.trackers_json", trackers_json.as_deref())?;

            let mut state = TorrentState::new(source, info_hash);
            state.auto_extract = decode_bool_i64(
                "torrent_state.auto_extract",
                require_i64("torrent_state.auto_extract", auto_extract)?,
            )?;
            state.is_private = decode_bool_i64(
                "torrent_state.is_private",
                require_i64("torrent_state.is_private", is_private)?,
            )?;
            state.files = files;
            state.trackers = trackers;
            state.metadata_bytes = metadata_bytes;
            state.uploaded = decode_u64(
                "torrent_state.uploaded",
                require_i64("torrent_state.uploaded", uploaded)?,
            )?;
            state.ratio = decode_ratio(
                "torrent_state.ratio",
                require_f64("torrent_state.ratio", ratio)?,
            )?;
            state.seed_elapsed_secs = decode_u64(
                "torrent_state.seed_elapsed_secs",
                require_i64("torrent_state.seed_elapsed_secs", seed_elapsed_secs)?,
            )?;

            DownloadKind::Torrent(state)
        },
        other => {
            return Err(ShioError::Other(format!("unknown download kind: {other}")));
        },
    };

    Ok(Download {
        id: DownloadId(id),
        filename,
        save_path: PathBuf::from(save_path),
        total_size: decode_optional_u64("downloads.total_size", total_size)?,
        downloaded: decode_u64("downloads.downloaded", downloaded)?,
        status: parse_download_status("downloads.status", &status_str)?,
        priority,
        speed: 0,
        avg_speed: 0,
        error_message,
        created_at: parse_required_ts("downloads.created_at", &created_at)?,
        started_at: parse_optional_ts("downloads.started_at", started_at.as_deref())?,
        completed_at: parse_optional_ts("downloads.completed_at", completed_at.as_deref())?,
        retry_count: decode_u32("downloads.retry_count", retry_count)?,
        max_retries: decode_u32("downloads.max_retries", max_retries)?,
        pinned: decode_bool_i32("downloads.pinned", pinned)?,
        kind,
    })
}

pub(super) fn row_to_chunk(row: &rusqlite::Row<'_>) -> Result<ChunkInfo> {
    let status_str: String = row.get(4)?;
    Ok(ChunkInfo {
        index: decode_u32("chunks.chunk_index", row.get::<_, i32>(0)?)?,
        start: decode_u64("chunks.start_byte", row.get::<_, i64>(1)?)?,
        end: decode_u64("chunks.end_byte", row.get::<_, i64>(2)?)?,
        downloaded: decode_u64("chunks.downloaded", row.get::<_, i64>(3)?)?,
        status: parse_chunk_status("chunks.status", &status_str)?,
    })
}

fn parse_download_status(field: &'static str, value: &str) -> Result<DownloadStatus> {
    let status = match value {
        "pending" => DownloadStatus::Pending,
        "queued" => DownloadStatus::Queued,
        "starting" => DownloadStatus::Starting,
        "downloading" => DownloadStatus::Downloading,
        "fetching_metadata" => DownloadStatus::FetchingMetadata,
        "extracting" => DownloadStatus::Extracting,
        "paused" => DownloadStatus::Paused,
        "seeding" => DownloadStatus::Seeding,
        "completed" => DownloadStatus::Completed,
        "error" => DownloadStatus::Error,
        "extract_error" => DownloadStatus::ExtractError,
        "password_required" => DownloadStatus::PasswordRequired,
        "cancelled" => DownloadStatus::Cancelled,
        other => {
            return Err(ShioError::DatabaseValue {
                field,
                value: other.to_string(),
            });
        },
    };
    Ok(status)
}

fn parse_chunk_status(field: &'static str, value: &str) -> Result<ChunkStatus> {
    let status = match value {
        "pending" => ChunkStatus::Pending,
        "downloading" => ChunkStatus::Downloading,
        "completed" => ChunkStatus::Completed,
        "error" => ChunkStatus::Error,
        other => {
            return Err(ShioError::DatabaseValue {
                field,
                value: other.to_string(),
            });
        },
    };
    Ok(status)
}
