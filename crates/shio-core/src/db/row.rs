use std::path::PathBuf;

use chrono::{DateTime, Utc};
use rusqlite::Row;

use super::codec::{
    decode_bool, decode_int, decode_json, decode_ratio, decode_ts, decode_uuid, invalid,
};
use crate::error::Result;
use crate::path::{validate_leaf_filename, validate_relative_path};
use crate::types::{
    ChunkInfo, ChunkStatus, Download, DownloadId, DownloadKind, DownloadStatus, HttpState,
    TorrentFile, TorrentSource, TorrentState, TorrentStats,
};

pub(super) const DOWNLOAD_SELECT: &str = "SELECT
    d.id, d.filename, d.save_path, d.total_size, d.downloaded, d.status,
    d.error_msg, d.created_at, d.started_at, d.completed_at,
    d.retry_count, d.max_retries, d.pinned, d.kind,
    h.url, h.headers, h.segments, h.subfolder, h.auto_extract,
    t.source_kind, t.source_data, t.info_hash, t.auto_extract, t.is_private,
    t.files_json, t.trackers_json, t.uploaded, t.ratio, t.seed_elapsed_secs, t.metadata_bytes
    FROM downloads d
    LEFT JOIN http_state h ON h.download_id = d.id
    LEFT JOIN torrent_state t ON t.download_id = d.id";

pub(super) fn row_to_download(row: &Row<'_>) -> Result<Download> {
    let filename: String = row.get(1)?;
    validate_leaf_filename(&filename)?;
    let kind = match row.get::<_, String>(13)?.as_str() {
        "http" => DownloadKind::Http(http_state(row)?),
        "torrent" => DownloadKind::Torrent(torrent_state(row)?),
        other => return Err(invalid("downloads.kind", &other)),
    };
    Ok(Download {
        id: DownloadId(decode_uuid("downloads.id", &row.get::<_, String>(0)?)?),
        filename,
        save_path: PathBuf::from(row.get::<_, String>(2)?),
        total_size: row
            .get::<_, Option<i64>>(3)?
            .map(|size| decode_int("downloads.total_size", size))
            .transpose()?,
        downloaded: decode_int("downloads.downloaded", row.get(4)?)?,
        status: parse_download_status(&row.get::<_, String>(5)?)?,
        speed: 0,
        error_message: row.get(6)?,
        created_at: decode_ts("downloads.created_at", &row.get::<_, String>(7)?)?,
        started_at: optional_ts(row, 8, "downloads.started_at")?,
        completed_at: optional_ts(row, 9, "downloads.completed_at")?,
        retry_count: decode_int("downloads.retry_count", row.get(10)?)?,
        max_retries: decode_int("downloads.max_retries", row.get(11)?)?,
        pinned: decode_bool("downloads.pinned", row.get(12)?)?,
        kind,
    })
}

// The kind tables' columns are NOT NULL, so a NULL key column means the joined row is missing.
fn http_state(row: &Row<'_>) -> Result<HttpState> {
    let url: String = row
        .get::<_, Option<String>>(14)?
        .ok_or_else(|| invalid("http_state", &"missing"))?;
    let subfolder: Option<String> = row.get(17)?;
    if let Some(subfolder) = subfolder.as_deref() {
        validate_leaf_filename(subfolder)?;
    }
    Ok(HttpState {
        url,
        headers: decode_json("http_state.headers", &row.get::<_, String>(15)?)?,
        segments: decode_int("http_state.segments", row.get(16)?)?,
        subfolder,
        auto_extract: decode_bool("http_state.auto_extract", row.get(18)?)?,
    })
}

fn torrent_state(row: &Row<'_>) -> Result<TorrentState> {
    let source_kind: String = row
        .get::<_, Option<String>>(19)?
        .ok_or_else(|| invalid("torrent_state", &"missing"))?;
    let source_data: Vec<u8> = row.get(20)?;
    let source = match source_kind.as_str() {
        "magnet" => TorrentSource::Magnet(
            String::from_utf8(source_data)
                .map_err(|_| invalid("torrent_state.source_data", &"non-UTF-8 magnet"))?,
        ),
        "file" => TorrentSource::File(source_data),
        other => return Err(invalid("torrent_state.source_kind", &other)),
    };
    let info_hash: Vec<u8> = row.get(21)?;
    let info_hash = <[u8; 20]>::try_from(info_hash.as_slice())
        .map_err(|_| invalid("torrent_state.info_hash", &info_hash.len()))?;
    let files: Vec<TorrentFile> =
        decode_json("torrent_state.files_json", &row.get::<_, String>(24)?)?;
    for file in &files {
        validate_relative_path(&file.path)?;
    }
    let mut state = TorrentState::new(source, info_hash);
    state.auto_extract = decode_bool("torrent_state.auto_extract", row.get(22)?)?;
    state.is_private = decode_bool("torrent_state.is_private", row.get(23)?)?;
    state.files = files;
    state.trackers = decode_json("torrent_state.trackers_json", &row.get::<_, String>(25)?)?;
    state.stats = TorrentStats {
        uploaded: decode_int("torrent_state.uploaded", row.get(26)?)?,
        ratio: decode_ratio("torrent_state.ratio", row.get(27)?)?,
        seed_elapsed_secs: decode_int("torrent_state.seed_elapsed_secs", row.get(28)?)?,
        ..TorrentStats::default()
    };
    state.metadata_bytes = row.get(29)?;
    Ok(state)
}

pub(super) fn optional_ts(
    row: &Row<'_>,
    index: usize,
    field: &'static str,
) -> Result<Option<DateTime<Utc>>> {
    row.get::<_, Option<String>>(index)?
        .map(|ts| decode_ts(field, &ts))
        .transpose()
}

pub(super) fn row_to_chunk(row: &Row<'_>) -> Result<ChunkInfo> {
    Ok(ChunkInfo {
        index: decode_int("chunks.chunk_index", row.get(0)?)?,
        start: decode_int("chunks.start_byte", row.get(1)?)?,
        end: decode_int("chunks.end_byte", row.get(2)?)?,
        downloaded: decode_int("chunks.downloaded", row.get(3)?)?,
        status: match row.get::<_, String>(4)?.as_str() {
            "pending" => ChunkStatus::Pending,
            "downloading" => ChunkStatus::Downloading,
            "completed" => ChunkStatus::Completed,
            other => return Err(invalid("chunks.status", &other)),
        },
    })
}

fn parse_download_status(value: &str) -> Result<DownloadStatus> {
    use DownloadStatus::{
        Cancelled, Completed, Downloading, Error, ExtractError, Extracting, FetchingMetadata,
        PasswordRequired, Paused, Pending, Queued, Seeding, Starting,
    };
    [
        Pending,
        Queued,
        Starting,
        Downloading,
        FetchingMetadata,
        Extracting,
        Paused,
        Seeding,
        Completed,
        Error,
        ExtractError,
        PasswordRequired,
        Cancelled,
    ]
    .into_iter()
    .find(|status| status.as_str() == value)
    .ok_or_else(|| invalid("downloads.status", &value))
}
