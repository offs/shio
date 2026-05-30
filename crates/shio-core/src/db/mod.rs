mod codec;
mod row;
mod schema;

use self::codec::{
    decode_bool_i32, decode_u32, encode_i32, encode_i64, encode_json, parse_optional_ts,
    parse_required_ts,
};
#[cfg(test)]
use self::row::DOWNLOAD_SELECT;
use self::row::{DOWNLOAD_SELECT_ALL, row_to_chunk, row_to_download};
use self::schema::{SCHEMA, SCHEMA_VERSION, require_current_schema, set_schema_version};
use crate::error::Result;
use crate::path::path_to_utf8;
use crate::types::{
    ArchivePackage, ChunkInfo, ChunkStatus, Download, DownloadId, DownloadKind, DownloadStatus,
    PackageExtractState, PackageId, PackageItem, PackageKind, TorrentFile, TorrentSource,
    is_sensitive_header,
};
use parking_lot::Mutex;
use rusqlite::{Connection, Transaction, params};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(crate) struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let allow_empty_schema = path == Path::new(":memory:")
            || !path.exists()
            || std::fs::metadata(path).is_ok_and(|metadata| metadata.len() == 0);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout=5000;
             PRAGMA temp_store=MEMORY;",
        )?;
        require_current_schema(&conn, allow_empty_schema)?;
        conn.execute_batch(SCHEMA)?;
        set_schema_version(&conn, SCHEMA_VERSION)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    #[cfg(test)]
    pub(crate) fn in_memory() -> Result<Self> {
        Self::open(Path::new(":memory:"))
    }

    pub(crate) fn checkpoint(&self) -> Result<()> {
        self.conn
            .lock()
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    pub(crate) fn insert_download(&self, d: &Download) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        insert_download_tx(&tx, d)?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn update_status(
        &self,
        id: DownloadId,
        status: DownloadStatus,
        error: Option<&str>,
    ) -> Result<()> {
        self.conn.lock().execute(
            "UPDATE downloads SET status=?2, error_msg=?3 WHERE id=?1",
            params![id.0.to_string(), status.as_str(), error],
        )?;
        Ok(())
    }

    pub(crate) fn update_final(
        &self,
        id: DownloadId,
        status: DownloadStatus,
        downloaded: u64,
        total_size: Option<u64>,
    ) -> Result<()> {
        let downloaded = encode_i64("downloads.downloaded", downloaded)?;
        let total_size = total_size
            .map(|value| encode_i64("downloads.total_size", value))
            .transpose()?;
        self.conn.lock().execute(
            "UPDATE downloads SET status=?2, downloaded=?3, total_size=?4 WHERE id=?1",
            params![id.0.to_string(), status.as_str(), downloaded, total_size,],
        )?;
        Ok(())
    }

    pub(crate) fn update_metadata(
        &self,
        id: DownloadId,
        filename: &str,
        save_path: &Path,
    ) -> Result<()> {
        self.conn.lock().execute(
            "UPDATE downloads SET filename=?2, save_path=?3 WHERE id=?1",
            params![
                id.0.to_string(),
                filename,
                path_to_utf8("downloads.save_path", save_path)?,
            ],
        )?;
        Ok(())
    }

    pub(crate) fn update_torrent_runtime(
        &self,
        id: DownloadId,
        uploaded: u64,
        ratio: f32,
        seed_elapsed_secs: u64,
    ) -> Result<()> {
        let uploaded = encode_i64("torrent_state.uploaded", uploaded)?;
        let seed_elapsed_secs = encode_i64("torrent_state.seed_elapsed_secs", seed_elapsed_secs)?;
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE torrent_state
             SET uploaded = ?2, ratio = ?3, seed_elapsed_secs = ?4
             WHERE download_id = ?1",
            params![
                id.0.to_string(),
                uploaded,
                f64::from(ratio),
                seed_elapsed_secs,
            ],
        )?;
        Ok(())
    }

    pub(crate) fn update_torrent_metadata(
        &self,
        id: DownloadId,
        is_private: bool,
        files: &[TorrentFile],
        trackers: &[String],
    ) -> Result<()> {
        let files_json = encode_json("torrent_state.files_json", files)?;
        let trackers_json = encode_json("torrent_state.trackers_json", trackers)?;
        self.conn.lock().execute(
            "UPDATE torrent_state
             SET is_private = ?2, files_json = ?3, trackers_json = ?4
             WHERE download_id = ?1",
            params![
                id.0.to_string(),
                i32::from(is_private),
                files_json,
                trackers_json,
            ],
        )?;
        Ok(())
    }

    pub(crate) fn update_pin(&self, id: DownloadId, pinned: bool) -> Result<()> {
        self.conn.lock().execute(
            "UPDATE downloads SET pinned=?2 WHERE id=?1",
            params![id.0.to_string(), i32::from(pinned)],
        )?;
        Ok(())
    }

    pub(crate) fn update_package_extract_state(
        &self,
        id: PackageId,
        state: PackageExtractState,
        error: Option<&str>,
    ) -> Result<()> {
        self.conn.lock().execute(
            "UPDATE packages
             SET extract_state=?2, error_msg=?3,
                 completed_at=CASE WHEN ?2='completed' THEN ?4 ELSE completed_at END
             WHERE id=?1",
            params![
                id.0.to_string(),
                state.as_str(),
                error,
                chrono::Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub(crate) fn delete_package(&self, id: PackageId) -> Result<()> {
        self.conn.lock().execute(
            "DELETE FROM packages WHERE id=?1",
            params![id.0.to_string()],
        )?;
        Ok(())
    }

    pub(crate) fn update_progress(
        &self,
        id: DownloadId,
        downloaded: u64,
        total_size: Option<u64>,
    ) -> Result<()> {
        let downloaded = encode_i64("downloads.downloaded", downloaded)?;
        let total_size = total_size
            .map(|value| encode_i64("downloads.total_size", value))
            .transpose()?;
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare_cached("UPDATE downloads SET downloaded=?2, total_size=?3 WHERE id=?1")?;
        stmt.execute(params![id.0.to_string(), downloaded, total_size,])?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn get(&self, id: DownloadId) -> Result<Option<Download>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(DOWNLOAD_SELECT)?;
        let mut rows = stmt.query(params![id.0.to_string()])?;
        match rows.next()? {
            Some(row) => Ok(Some(row_to_download(row)?)),
            None => Ok(None),
        }
    }

    pub(crate) fn all(&self) -> Result<Vec<Download>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&format!("{DOWNLOAD_SELECT_ALL} ORDER BY created_at DESC"))?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(row_to_download(row)?);
        }
        Ok(out)
    }

    pub(crate) fn insert_archive_package(
        &self,
        package: &ArchivePackage,
        downloads: &[Download],
    ) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO packages (
                id, name, save_path, kind, auto_extract, extract_state,
                error_msg, created_at, completed_at, pinned
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                package.id.0.to_string(),
                package.name,
                path_to_utf8("packages.save_path", &package.save_path)?,
                package.kind.as_str(),
                i32::from(package.auto_extract),
                package.extract_state.as_str(),
                package.error_message,
                package.created_at.to_rfc3339(),
                package.completed_at.map(|t| t.to_rfc3339()),
                i32::from(package.pinned),
            ],
        )?;
        for download in downloads {
            insert_download_tx(&tx, download)?;
        }
        for item in &package.items {
            tx.execute(
                "INSERT INTO package_items (package_id, download_id, position, part_number)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    package.id.0.to_string(),
                    item.download_id.0.to_string(),
                    encode_i32("package_items.position", item.position)?,
                    encode_i32("package_items.part_number", item.part_number)?,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn packages(&self) -> Result<Vec<ArchivePackage>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, save_path, kind, auto_extract, extract_state,
                    error_msg, created_at, completed_at, pinned
             FROM packages ORDER BY created_at DESC",
        )?;
        let mut rows = stmt.query([])?;
        let mut packages = Vec::new();
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let package_id =
                PackageId(uuid::Uuid::parse_str(&id).map_err(|e| {
                    crate::ShioError::Other(format!("invalid package uuid {id}: {e}"))
                })?);
            let name: String = row.get(1)?;
            crate::path::validate_leaf_filename(&name)?;
            let save_path: String = row.get(2)?;
            let kind: String = row.get(3)?;
            let kind = match kind.as_str() {
                "archive_set" => PackageKind::ArchiveSet,
                other => {
                    return Err(crate::ShioError::DatabaseValue {
                        field: "packages.kind",
                        value: other.to_string(),
                    });
                },
            };
            let auto_extract: i32 = row.get(4)?;
            let extract_state: String = row.get(5)?;
            let extract_state = parse_package_extract_state(&extract_state)?;
            let error_message: Option<String> = row.get(6)?;
            let created_at: String = row.get(7)?;
            let completed_at: Option<String> = row.get(8)?;
            let pinned: i32 = row.get(9)?;
            packages.push(ArchivePackage {
                id: package_id,
                name,
                save_path: PathBuf::from(save_path),
                kind,
                auto_extract: decode_bool_i32("packages.auto_extract", auto_extract)?,
                extract_state,
                error_message,
                created_at: parse_required_ts("packages.created_at", &created_at)?,
                completed_at: parse_optional_ts("packages.completed_at", completed_at.as_deref())?,
                pinned: decode_bool_i32("packages.pinned", pinned)?,
                items: package_items_locked(&conn, package_id)?,
            });
        }
        Ok(packages)
    }

    pub(crate) fn delete(&self, id: DownloadId) -> Result<()> {
        self.conn.lock().execute(
            "DELETE FROM downloads WHERE id=?1",
            params![id.0.to_string()],
        )?;
        Ok(())
    }

    pub(crate) fn insert_chunks(&self, id: DownloadId, chunks: &[ChunkInfo]) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO chunks
                 (download_id, chunk_index, start_byte, end_byte, downloaded, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for c in chunks {
                let index = encode_i32("chunks.chunk_index", c.index)?;
                let start = encode_i64("chunks.start_byte", c.start)?;
                let end = encode_i64("chunks.end_byte", c.end)?;
                let downloaded = encode_i64("chunks.downloaded", c.downloaded)?;
                stmt.execute(params![
                    id.0.to_string(),
                    index,
                    start,
                    end,
                    downloaded,
                    c.status.as_str(),
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn update_chunk(
        &self,
        id: DownloadId,
        index: u32,
        downloaded: u64,
        status: ChunkStatus,
    ) -> Result<()> {
        let index = encode_i32("chunks.chunk_index", index)?;
        let downloaded = encode_i64("chunks.downloaded", downloaded)?;
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached(
            "UPDATE chunks SET downloaded=?3, status=?4
             WHERE download_id=?1 AND chunk_index=?2",
        )?;
        stmt.execute(params![
            id.0.to_string(),
            index,
            downloaded,
            status.as_str(),
        ])?;
        Ok(())
    }

    pub(crate) fn chunks(&self, id: DownloadId) -> Result<Vec<ChunkInfo>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT chunk_index, start_byte, end_byte, downloaded, status
             FROM chunks WHERE download_id=?1 ORDER BY chunk_index",
        )?;
        let mut rows = stmt.query(params![id.0.to_string()])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(row_to_chunk(row)?);
        }
        Ok(out)
    }

    pub(crate) fn delete_chunks(&self, id: DownloadId) -> Result<()> {
        self.conn.lock().execute(
            "DELETE FROM chunks WHERE download_id=?1",
            params![id.0.to_string()],
        )?;
        Ok(())
    }
}

fn insert_download_tx(tx: &Transaction<'_>, d: &Download) -> Result<()> {
    let total_size = d
        .total_size
        .map(|value| encode_i64("downloads.total_size", value))
        .transpose()?;
    let downloaded = encode_i64("downloads.downloaded", d.downloaded)?;
    let retry_count = encode_i32("downloads.retry_count", d.retry_count)?;
    let max_retries = encode_i32("downloads.max_retries", d.max_retries)?;
    tx.execute(
        "INSERT INTO downloads (
            id, filename, save_path, total_size, downloaded, status,
            priority, error_msg, created_at, started_at, completed_at,
            retry_count, max_retries, pinned, kind
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15
         )",
        params![
            d.id.0.to_string(),
            d.filename,
            path_to_utf8("downloads.save_path", &d.save_path)?,
            total_size,
            downloaded,
            d.status.as_str(),
            d.priority,
            d.error_message,
            d.created_at.to_rfc3339(),
            d.started_at.map(|t| t.to_rfc3339()),
            d.completed_at.map(|t| t.to_rfc3339()),
            retry_count,
            max_retries,
            i32::from(d.pinned),
            d.kind.as_str(),
        ],
    )?;
    match &d.kind {
        DownloadKind::Http(http) => {
            validate_persisted_headers(&http.headers)?;
            let headers_json = encode_json("http_state.headers", &http.headers)?;
            tx.execute(
                "INSERT INTO http_state (download_id, url, headers, segments, subfolder, auto_extract)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    d.id.0.to_string(),
                    http.url,
                    headers_json,
                    i32::from(http.segments),
                    http.subfolder.as_deref(),
                    i32::from(http.auto_extract),
                ],
            )?;
        },
        DownloadKind::Torrent(t) => {
            let (source_kind, source_data): (&str, Vec<u8>) = match &t.source {
                TorrentSource::Magnet(m) => ("magnet", m.as_bytes().to_vec()),
                TorrentSource::File(bytes) => ("file", bytes.clone()),
            };
            let files_json = encode_json("torrent_state.files_json", &t.files)?;
            let trackers_json = encode_json("torrent_state.trackers_json", &t.trackers)?;
            let uploaded = encode_i64("torrent_state.uploaded", t.uploaded)?;
            let seed_elapsed_secs =
                encode_i64("torrent_state.seed_elapsed_secs", t.seed_elapsed_secs)?;
            tx.execute(
                "INSERT INTO torrent_state (
                    download_id, source_kind, source_data, info_hash, auto_extract, is_private,
                    files_json, trackers_json, uploaded, ratio, seed_elapsed_secs, metadata_bytes
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    d.id.0.to_string(),
                    source_kind,
                    source_data,
                    t.info_hash.to_vec(),
                    i32::from(t.auto_extract),
                    i32::from(t.is_private),
                    files_json,
                    trackers_json,
                    uploaded,
                    f64::from(t.ratio),
                    seed_elapsed_secs,
                    t.metadata_bytes,
                ],
            )?;
        },
    }
    Ok(())
}

fn package_items_locked(conn: &Connection, package_id: PackageId) -> Result<Vec<PackageItem>> {
    let mut stmt = conn.prepare(
        "SELECT download_id, position, part_number
         FROM package_items WHERE package_id=?1 ORDER BY position",
    )?;
    let mut rows = stmt.query(params![package_id.0.to_string()])?;
    let mut items = Vec::new();
    while let Some(row) = rows.next()? {
        let download_id: String = row.get(0)?;
        let download_id = DownloadId(uuid::Uuid::parse_str(&download_id).map_err(|e| {
            crate::ShioError::Other(format!("invalid package item uuid {download_id}: {e}"))
        })?);
        let position: i32 = row.get(1)?;
        let part_number: i32 = row.get(2)?;
        items.push(PackageItem {
            download_id,
            position: decode_u32("package_items.position", position)?,
            part_number: decode_u32("package_items.part_number", part_number)?,
        });
    }
    Ok(items)
}

fn parse_package_extract_state(value: &str) -> Result<PackageExtractState> {
    let state = match value {
        "not_started" => PackageExtractState::NotStarted,
        "extracting" => PackageExtractState::Extracting,
        "completed" => PackageExtractState::Completed,
        "error" => PackageExtractState::Error,
        "password_required" => PackageExtractState::PasswordRequired,
        other => {
            return Err(crate::ShioError::DatabaseValue {
                field: "packages.extract_state",
                value: other.to_string(),
            });
        },
    };
    Ok(state)
}

fn validate_persisted_headers(headers: &[(String, String)]) -> Result<()> {
    if let Some((name, _)) = headers
        .iter()
        .find(|(name, _)| is_sensitive_header(name.as_str()))
    {
        return Err(crate::ShioError::DatabaseValue {
            field: "http_state.headers",
            value: format!("sensitive header cannot be persisted: {name}"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample() -> Download {
        Download::new(
            "https://example.com/file.zip".to_string(),
            PathBuf::from("/tmp/file.zip"),
        )
    }

    fn db() -> Database {
        Database::in_memory().unwrap()
    }

    #[test]
    fn open_creates_schema() {
        let _ = db();
    }

    #[test]
    fn insert_and_get_roundtrip() {
        let db = db();
        let d = sample();
        db.insert_download(&d).unwrap();
        let got = db.get(d.id).unwrap().unwrap();
        assert_eq!(got.id, d.id);
        assert_eq!(got.url(), d.url());
    }

    #[test]
    fn get_missing_returns_none() {
        assert!(db().get(DownloadId::new()).unwrap().is_none());
    }

    #[test]
    fn update_status_persists() {
        let db = db();
        let d = sample();
        db.insert_download(&d).unwrap();
        db.update_status(d.id, DownloadStatus::Completed, None)
            .unwrap();
        assert_eq!(
            db.get(d.id).unwrap().unwrap().status,
            DownloadStatus::Completed
        );
    }

    #[test]
    fn update_progress_persists() {
        let db = db();
        let d = sample();
        db.insert_download(&d).unwrap();
        db.update_progress(d.id, 12_345, Some(100_000)).unwrap();
        let got = db.get(d.id).unwrap().unwrap();
        assert_eq!(got.downloaded, 12_345);
        assert_eq!(got.total_size, Some(100_000));
    }

    #[test]
    fn delete_cascades_chunks() {
        let db = db();
        let d = sample();
        db.insert_download(&d).unwrap();
        let chunks = vec![ChunkInfo {
            index: 0,
            start: 0,
            end: 99,
            downloaded: 0,
            status: ChunkStatus::Pending,
        }];
        db.insert_chunks(d.id, &chunks).unwrap();
        db.delete(d.id).unwrap();
        assert!(db.chunks(d.id).unwrap().is_empty());
    }

    #[test]
    fn all_returns_inserted() {
        let db = db();
        db.insert_download(&sample()).unwrap();
        db.insert_download(&sample()).unwrap();
        assert_eq!(db.all().unwrap().len(), 2);
    }

    #[test]
    fn chunks_roundtrip_with_update() {
        let db = db();
        let d = sample();
        db.insert_download(&d).unwrap();
        let chunks = vec![
            ChunkInfo {
                index: 0,
                start: 0,
                end: 99,
                downloaded: 0,
                status: ChunkStatus::Pending,
            },
            ChunkInfo {
                index: 1,
                start: 100,
                end: 199,
                downloaded: 50,
                status: ChunkStatus::Downloading,
            },
        ];
        db.insert_chunks(d.id, &chunks).unwrap();
        db.update_chunk(d.id, 0, 100, ChunkStatus::Completed)
            .unwrap();

        let got = db.chunks(d.id).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].downloaded, 100);
        assert_eq!(got[0].status, ChunkStatus::Completed);
        assert_eq!(got[1].downloaded, 50);
    }

    #[test]
    fn pin_roundtrips() {
        let db = db();
        let d = sample();
        db.insert_download(&d).unwrap();
        assert!(!db.get(d.id).unwrap().unwrap().pinned);
        db.update_pin(d.id, true).unwrap();
        assert!(db.get(d.id).unwrap().unwrap().pinned);
    }

    #[test]
    fn metadata_update_preserves_status() {
        let db = db();
        let mut d = sample();
        d.status = DownloadStatus::Completed;
        db.insert_download(&d).unwrap();
        db.update_metadata(d.id, "x.mkv", Path::new("/tmp"))
            .unwrap();
        let got = db.get(d.id).unwrap().unwrap();
        assert_eq!(got.status, DownloadStatus::Completed);
        assert_eq!(got.filename, "x.mkv");
    }

    #[test]
    fn subfolder_and_auto_extract_roundtrip() {
        let db = db();
        let mut d = sample();
        if let Some(http) = d.http_mut() {
            http.subfolder = Some("pkg".into());
            http.auto_extract = true;
        }
        db.insert_download(&d).unwrap();
        let got = db.get(d.id).unwrap().unwrap();
        assert_eq!(
            got.http().and_then(|h| h.subfolder.clone()),
            Some("pkg".into())
        );
        assert!(got.http().is_some_and(|h| h.auto_extract));
    }

    #[test]
    fn invalid_http_headers_json_returns_error() {
        let db = db();
        let d = sample();
        db.insert_download(&d).unwrap();
        db.conn
            .lock()
            .execute(
                "UPDATE http_state SET headers = ?2 WHERE download_id = ?1",
                params![d.id.0.to_string(), "not json"],
            )
            .unwrap();

        assert!(db.get(d.id).is_err());
    }

    #[test]
    fn sensitive_http_headers_are_not_persisted() {
        let db = db();
        let mut d = sample();
        d.http_mut().unwrap().headers = vec![("Authorization".to_string(), "secret".to_string())];

        assert!(db.insert_download(&d).is_err());
    }

    #[test]
    fn missing_http_headers_json_returns_error() {
        let db = db();
        let d = sample();
        db.insert_download(&d).unwrap();
        let result = db.conn.lock().execute(
            "UPDATE http_state SET headers = NULL WHERE download_id = ?1",
            params![d.id.0.to_string()],
        );

        assert!(result.is_err());
    }

    #[test]
    fn all_returns_error_for_corrupt_download_row() {
        let db = db();
        let d = sample();
        db.insert_download(&d).unwrap();
        db.conn
            .lock()
            .execute(
                "UPDATE http_state SET headers = ?2 WHERE download_id = ?1",
                params![d.id.0.to_string(), "not json"],
            )
            .unwrap();

        assert!(db.all().is_err());
    }

    #[test]
    fn invalid_created_at_returns_error() {
        let db = db();
        let d = sample();
        db.insert_download(&d).unwrap();
        db.conn
            .lock()
            .execute(
                "UPDATE downloads SET created_at = ?2 WHERE id = ?1",
                params![d.id.0.to_string(), "not a timestamp"],
            )
            .unwrap();

        assert!(db.get(d.id).is_err());
    }

    #[test]
    fn negative_downloaded_bytes_return_error() {
        let db = db();
        let d = sample();
        db.insert_download(&d).unwrap();
        let result = db.conn.lock().execute(
            "UPDATE downloads SET downloaded = ?2 WHERE id = ?1",
            params![d.id.0.to_string(), -1],
        );

        assert!(result.is_err());
    }

    #[test]
    fn invalid_download_status_returns_error() {
        let db = db();
        let d = sample();
        db.insert_download(&d).unwrap();
        let result = db.conn.lock().execute(
            "UPDATE downloads SET status = ?2 WHERE id = ?1",
            params![d.id.0.to_string(), "not_a_status"],
        );

        assert!(result.is_err());
    }

    #[test]
    fn invalid_chunk_status_returns_error() {
        let db = db();
        let d = sample();
        db.insert_download(&d).unwrap();
        db.insert_chunks(
            d.id,
            &[ChunkInfo {
                index: 0,
                start: 0,
                end: 99,
                downloaded: 0,
                status: ChunkStatus::Pending,
            }],
        )
        .unwrap();
        let result = db.conn.lock().execute(
            "UPDATE chunks SET status = ?2 WHERE download_id = ?1",
            params![d.id.0.to_string(), "not_a_status"],
        );

        assert!(result.is_err());
    }

    #[test]
    fn invalid_http_segments_return_error() {
        let db = db();
        let d = sample();
        db.insert_download(&d).unwrap();
        let result = db.conn.lock().execute(
            "UPDATE http_state SET segments = ?2 WHERE download_id = ?1",
            params![d.id.0.to_string(), 256],
        );

        assert!(result.is_err());
    }

    #[test]
    fn unsafe_filename_in_download_row_returns_error() {
        let db = db();
        let d = sample();
        db.insert_download(&d).unwrap();
        db.conn
            .lock()
            .execute(
                "UPDATE downloads SET filename = ?2 WHERE id = ?1",
                params![d.id.0.to_string(), "../file.bin"],
            )
            .unwrap();

        assert!(db.get(d.id).is_err());
    }

    #[test]
    fn unsafe_http_subfolder_in_row_returns_error() {
        let db = db();
        let d = sample();
        db.insert_download(&d).unwrap();
        db.conn
            .lock()
            .execute(
                "UPDATE http_state SET subfolder = ?2 WHERE download_id = ?1",
                params![d.id.0.to_string(), "../release"],
            )
            .unwrap();

        assert!(db.get(d.id).is_err());
    }

    #[test]
    fn corrupt_chunk_row_returns_error() {
        let db = db();
        let d = sample();
        db.insert_download(&d).unwrap();
        db.insert_chunks(
            d.id,
            &[ChunkInfo {
                index: 0,
                start: 0,
                end: 99,
                downloaded: 0,
                status: ChunkStatus::Pending,
            }],
        )
        .unwrap();
        let result = db.conn.lock().execute(
            "UPDATE chunks SET downloaded = ?2 WHERE download_id = ?1",
            params![d.id.0.to_string(), -1],
        );

        assert!(result.is_err());
    }

    fn torrent_sample() -> Download {
        Download::try_from_torrent(
            TorrentSource::Magnet(
                "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862".into(),
            ),
            PathBuf::from("/tmp"),
        )
        .expect("valid magnet")
    }

    #[test]
    fn torrent_insert_and_get_roundtrip() {
        let db = db();
        let mut d = torrent_sample();
        d.torrent_mut().unwrap().auto_extract = true;
        d.torrent_mut().unwrap().metadata_bytes = Some(b"metadata".to_vec());
        db.insert_download(&d).unwrap();
        let got = db.get(d.id).unwrap().unwrap();
        assert!(got.kind.is_torrent());
        let t = got.torrent().unwrap();
        assert_eq!(t.info_hash, d.torrent().unwrap().info_hash);
        assert!(t.auto_extract);
        assert_eq!(t.metadata_bytes.as_deref(), Some(&b"metadata"[..]));
        match &t.source {
            TorrentSource::Magnet(m) => assert!(m.starts_with("magnet:")),
            TorrentSource::File(_) => panic!("expected magnet"),
        }
    }

    #[test]
    fn torrent_update_runtime_persists() {
        let db = db();
        let d = torrent_sample();
        db.insert_download(&d).unwrap();

        db.update_torrent_runtime(d.id, 2048, 0.75, 300).unwrap();

        let got = db.get(d.id).unwrap().unwrap();
        let t = got.torrent().unwrap();
        assert_eq!(t.uploaded, 2048);
        assert!((t.ratio - 0.75).abs() < 1e-5);
        assert_eq!(t.seed_elapsed_secs, 300);
    }

    #[test]
    fn missing_torrent_required_runtime_returns_error() {
        let db = db();
        let d = torrent_sample();
        db.insert_download(&d).unwrap();
        let result = db.conn.lock().execute(
            "UPDATE torrent_state SET uploaded = NULL WHERE download_id = ?1",
            params![d.id.0.to_string()],
        );

        assert!(result.is_err());
    }

    #[test]
    fn torrent_update_metadata_persists() {
        let db = db();
        let d = torrent_sample();
        db.insert_download(&d).unwrap();

        let files = vec![TorrentFile {
            path: PathBuf::from("release/archive.zip"),
            size: 1024,
            downloaded: 1024,
            selected: true,
        }];
        let trackers = vec!["udp://tracker.example:1337/announce".to_string()];
        db.update_torrent_metadata(d.id, true, &files, &trackers)
            .unwrap();

        let got = db.get(d.id).unwrap().unwrap();
        let torrent = got.torrent().unwrap();
        assert!(torrent.is_private);
        assert_eq!(torrent.files, files);
        assert_eq!(torrent.trackers, trackers);
    }

    #[test]
    fn unsafe_torrent_file_path_in_row_returns_error() {
        let db = db();
        let d = torrent_sample();
        db.insert_download(&d).unwrap();
        let files = serde_json::to_string(&[TorrentFile {
            path: PathBuf::from("../outside.bin"),
            size: 1,
            downloaded: 0,
            selected: true,
        }])
        .unwrap();
        db.conn
            .lock()
            .execute(
                "UPDATE torrent_state SET files_json = ?2 WHERE download_id = ?1",
                params![d.id.0.to_string(), files],
            )
            .unwrap();

        assert!(db.get(d.id).is_err());
    }

    #[test]
    fn torrent_and_http_coexist_in_all() {
        let db = db();
        db.insert_download(&sample()).unwrap();
        db.insert_download(&torrent_sample()).unwrap();
        let all = db.all().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all.iter().filter(|d| d.kind.is_http()).count(), 1);
        assert_eq!(all.iter().filter(|d| d.kind.is_torrent()).count(), 1);
    }

    #[test]
    fn schema_creates_side_tables() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let _db = Database::open(tmp.path()).unwrap();
        let conn = Connection::open(tmp.path()).unwrap();

        for table in ["downloads", "http_state", "torrent_state", "chunks"] {
            let exists: bool = conn
                .prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1")
                .unwrap()
                .exists([table])
                .unwrap();
            assert!(exists, "{table} must exist");
        }

        let has_kind: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('downloads') WHERE name='kind'")
            .unwrap()
            .exists([])
            .unwrap();
        assert!(has_kind, "downloads.kind must exist");

        let has_url_on_downloads: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('downloads') WHERE name='url'")
            .unwrap()
            .exists([])
            .unwrap();
        assert!(
            !has_url_on_downloads,
            "downloads.url must NOT exist (moved to http_state)"
        );
    }

    #[test]
    fn open_sets_schema_user_version() {
        let tmp = tempfile::NamedTempFile::new().unwrap();

        let _db = Database::open(tmp.path()).unwrap();

        let conn = Connection::open(tmp.path()).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }
}
