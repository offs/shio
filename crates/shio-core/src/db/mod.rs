mod codec;
mod row;
mod schema;

use self::codec::{
    decode_bool, decode_int, decode_json, decode_ts, decode_uuid, encode_json, encode_u64, invalid,
};
use self::row::{DOWNLOAD_SELECT, optional_ts, row_to_chunk, row_to_download};
use crate::error::Result;
use crate::path::path_to_utf8;
use crate::types::{
    ArchivePackage, ChunkInfo, Download, DownloadId, DownloadKind, PackageExtractState, PackageId,
    PackageItem, TorrentSource, TorrentStats, is_sensitive_header,
};
use parking_lot::Mutex;
use rusqlite::{Connection, Transaction, params};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(crate) struct Database {
    conn: Mutex<Connection>,
}

#[derive(Debug)]
pub(crate) struct DeleteJournal {
    pub(crate) download_id: DownloadId,
    pub(crate) protected_root: PathBuf,
    pub(crate) quarantine_root: PathBuf,
    pub(crate) moved: Vec<(PathBuf, PathBuf)>,
}

impl Database {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        if let Ok(metadata) = std::fs::symlink_metadata(path)
            && (!metadata.is_file() || metadata.file_type().is_symlink())
        {
            return Err(crate::ShioError::Other(
                "database path is not a real file".into(),
            ));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        #[cfg(unix)]
        restrict_to_owner(path)?;
        Self::init(conn, !cfg!(test))
    }

    #[cfg(test)]
    pub(crate) fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?, false)
    }

    fn init(mut conn: Connection, exclusive: bool) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout=5000;
             PRAGMA temp_store=MEMORY;",
        )?;
        if exclusive {
            conn.execute_batch("PRAGMA locking_mode=EXCLUSIVE; BEGIN IMMEDIATE; COMMIT;")?;
        }
        schema::ensure_schema(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub(crate) fn checkpoint(&self) -> Result<()> {
        self.conn
            .lock()
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    fn write(&self, f: impl FnOnce(&Transaction<'_>) -> Result<()>) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        f(&tx)?;
        tx.commit()?;
        Ok(())
    }

    fn execute(&self, sql: &str, params: impl rusqlite::Params) -> Result<()> {
        self.conn.lock().execute(sql, params)?;
        Ok(())
    }

    pub(crate) fn insert_download(&self, download: &Download) -> Result<()> {
        self.write(|tx| insert_download(tx, download))
    }

    pub(crate) fn insert_package(
        &self,
        package: &ArchivePackage,
        downloads: &[Download],
    ) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO packages (
                    id, name, save_path, auto_extract, extract_state,
                    error_msg, created_at, completed_at, pinned
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    package.id.0.to_string(),
                    package.name,
                    path_to_utf8("packages.save_path", &package.save_path)?,
                    package.auto_extract,
                    package.extract_state.as_str(),
                    package.error_message,
                    package.created_at.to_rfc3339(),
                    package.completed_at.map(|t| t.to_rfc3339()),
                    package.pinned,
                ],
            )?;
            for download in downloads {
                insert_download(tx, download)?;
            }
            for item in &package.items {
                tx.execute(
                    "INSERT INTO package_items (package_id, download_id, position, part_number)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        package.id.0.to_string(),
                        item.download_id.0.to_string(),
                        item.position,
                        item.part_number,
                    ],
                )?;
            }
            Ok(())
        })
    }

    pub(crate) fn update_downloads(&self, downloads: &[Download]) -> Result<()> {
        self.write(|tx| {
            for download in downloads {
                tx.execute(
                    "UPDATE downloads
                     SET status=?2, downloaded=?3, total_size=?4, error_msg=?5,
                         retry_count=?6, started_at=?7, completed_at=?8, pinned=?9
                     WHERE id=?1",
                    params![
                        download.id.0.to_string(),
                        download.status.as_str(),
                        encode_u64("downloads.downloaded", download.downloaded)?,
                        download
                            .total_size
                            .map(|size| encode_u64("downloads.total_size", size))
                            .transpose()?,
                        download.error_message,
                        download.retry_count,
                        download.started_at.map(|t| t.to_rfc3339()),
                        download.completed_at.map(|t| t.to_rfc3339()),
                        download.pinned,
                    ],
                )?;
                if let Some(torrent) = download.torrent() {
                    update_torrent_stats(tx, download.id, &torrent.stats)?;
                }
            }
            Ok(())
        })
    }

    pub(crate) fn update_progress(
        &self,
        id: DownloadId,
        downloaded: u64,
        total_size: Option<u64>,
        stats: Option<&TorrentStats>,
    ) -> Result<()> {
        self.write(|tx| {
            tx.prepare_cached("UPDATE downloads SET downloaded=?2, total_size=?3 WHERE id=?1")?
                .execute(params![
                    id.0.to_string(),
                    encode_u64("downloads.downloaded", downloaded)?,
                    total_size
                        .map(|size| encode_u64("downloads.total_size", size))
                        .transpose()?,
                ])?;
            if let Some(stats) = stats {
                update_torrent_stats(tx, id, stats)?;
            }
            Ok(())
        })
    }

    pub(crate) fn update_filename(&self, id: DownloadId, filename: &str) -> Result<()> {
        self.execute(
            "UPDATE downloads SET filename=?2 WHERE id=?1",
            params![id.0.to_string(), filename],
        )
    }

    pub(crate) fn update_pinned(&self, id: DownloadId, pinned: bool) -> Result<()> {
        self.execute(
            "UPDATE downloads SET pinned=?2 WHERE id=?1",
            params![id.0.to_string(), pinned],
        )
    }

    pub(crate) fn update_torrent_metadata(&self, download: &Download) -> Result<()> {
        let Some(torrent) = download.torrent() else {
            return Ok(());
        };
        self.write(|tx| {
            tx.execute(
                "UPDATE downloads SET filename=?2, total_size=?3 WHERE id=?1",
                params![
                    download.id.0.to_string(),
                    download.filename,
                    download
                        .total_size
                        .map(|size| encode_u64("downloads.total_size", size))
                        .transpose()?,
                ],
            )?;
            tx.execute(
                "UPDATE torrent_state
                 SET is_private=?2, files_json=?3, trackers_json=?4, metadata_bytes=?5
                 WHERE download_id=?1",
                params![
                    download.id.0.to_string(),
                    torrent.is_private,
                    encode_json("torrent_state.files_json", &torrent.files)?,
                    encode_json("torrent_state.trackers_json", &torrent.trackers)?,
                    torrent.metadata_bytes,
                ],
            )?;
            Ok(())
        })
    }

    pub(crate) fn update_package(&self, package: &ArchivePackage) -> Result<()> {
        self.execute(
            "UPDATE packages SET extract_state=?2, error_msg=?3, completed_at=?4, pinned=?5
             WHERE id=?1",
            params![
                package.id.0.to_string(),
                package.extract_state.as_str(),
                package.error_message,
                package.completed_at.map(|t| t.to_rfc3339()),
                package.pinned,
            ],
        )
    }

    fn select_downloads(&self, tail: &str, params: impl rusqlite::Params) -> Result<Vec<Download>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&format!("{DOWNLOAD_SELECT} {tail}"))?;
        stmt.query_and_then(params, row_to_download)?.collect()
    }

    /// Oldest first, so a restart requeues in insertion order.
    pub(crate) fn downloads(&self) -> Result<Vec<Download>> {
        self.select_downloads("ORDER BY d.created_at", [])
    }

    #[cfg(test)]
    pub(crate) fn get(&self, id: DownloadId) -> Result<Option<Download>> {
        Ok(self
            .select_downloads("WHERE d.id = ?1", [id.0.to_string()])?
            .pop())
    }

    pub(crate) fn packages(&self) -> Result<Vec<ArchivePackage>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, save_path, auto_extract, extract_state,
                    error_msg, created_at, completed_at, pinned
             FROM packages ORDER BY created_at",
        )?;
        stmt.query_and_then([], |row| {
            let id = PackageId(decode_uuid("packages.id", &row.get::<_, String>(0)?)?);
            let name: String = row.get(1)?;
            crate::path::validate_leaf_filename(&name)?;
            Ok(ArchivePackage {
                id,
                name,
                save_path: PathBuf::from(row.get::<_, String>(2)?),
                auto_extract: decode_bool("packages.auto_extract", row.get(3)?)?,
                extract_state: parse_extract_state(&row.get::<_, String>(4)?)?,
                error_message: row.get(5)?,
                created_at: decode_ts("packages.created_at", &row.get::<_, String>(6)?)?,
                completed_at: optional_ts(row, 7, "packages.completed_at")?,
                pinned: decode_bool("packages.pinned", row.get(8)?)?,
                items: package_items(&conn, id)?,
            })
        })?
        .collect()
    }

    pub(crate) fn delete_download(&self, id: DownloadId) -> Result<()> {
        self.execute("DELETE FROM downloads WHERE id=?1", [id.0.to_string()])
    }

    pub(crate) fn delete_package(&self, package: &ArchivePackage) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "DELETE FROM packages WHERE id=?1",
                [package.id.0.to_string()],
            )?;
            for id in package.child_ids() {
                tx.execute("DELETE FROM downloads WHERE id=?1", [id.0.to_string()])?;
            }
            Ok(())
        })
    }

    pub(crate) fn chunks(&self, id: DownloadId) -> Result<Vec<ChunkInfo>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT chunk_index, start_byte, end_byte, downloaded, status
             FROM chunks WHERE download_id=?1 ORDER BY chunk_index",
        )?;
        stmt.query_and_then([id.0.to_string()], row_to_chunk)?
            .collect()
    }

    pub(crate) fn update_chunks(&self, id: DownloadId, chunks: &[ChunkInfo]) -> Result<()> {
        self.write(|tx| {
            let mut stmt = tx.prepare_cached(
                "UPDATE chunks SET downloaded=?3, status=?4
                 WHERE download_id=?1 AND chunk_index=?2",
            )?;
            for chunk in chunks {
                stmt.execute(params![
                    id.0.to_string(),
                    chunk.index,
                    encode_u64("chunks.downloaded", chunk.downloaded)?,
                    chunk.status.as_str(),
                ])?;
            }
            Ok(())
        })
    }

    pub(crate) fn replace_chunks(&self, id: DownloadId, chunks: &[ChunkInfo]) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "DELETE FROM chunks WHERE download_id=?1",
                [id.0.to_string()],
            )?;
            let mut stmt = tx.prepare_cached(
                "INSERT INTO chunks
                 (download_id, chunk_index, start_byte, end_byte, downloaded, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for chunk in chunks {
                stmt.execute(params![
                    id.0.to_string(),
                    chunk.index,
                    encode_u64("chunks.start_byte", chunk.start)?,
                    encode_u64("chunks.end_byte", chunk.end)?,
                    encode_u64("chunks.downloaded", chunk.downloaded)?,
                    chunk.status.as_str(),
                ])?;
            }
            Ok(())
        })
    }

    pub(crate) fn insert_delete_journal(&self, journal: &DeleteJournal) -> Result<()> {
        let moved = journal
            .moved
            .iter()
            .map(|(original, staged)| {
                Ok((
                    path_to_utf8("delete_journals.original", original)?,
                    path_to_utf8("delete_journals.staged", staged)?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        self.execute(
            "INSERT INTO delete_journals (download_id, protected_root, quarantine_root, moved_json)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                journal.download_id.0.to_string(),
                path_to_utf8("delete_journals.protected_root", &journal.protected_root)?,
                path_to_utf8("delete_journals.quarantine_root", &journal.quarantine_root)?,
                encode_json("delete_journals.moved_json", &moved)?,
            ],
        )
    }

    pub(crate) fn delete_delete_journal(&self, id: DownloadId) -> Result<()> {
        self.execute(
            "DELETE FROM delete_journals WHERE download_id=?1",
            [id.0.to_string()],
        )
    }

    pub(crate) fn delete_journals(&self) -> Result<Vec<DeleteJournal>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT download_id, protected_root, quarantine_root, moved_json FROM delete_journals",
        )?;
        stmt.query_and_then([], |row| {
            Ok(DeleteJournal {
                download_id: DownloadId(decode_uuid(
                    "delete_journals.download_id",
                    &row.get::<_, String>(0)?,
                )?),
                protected_root: PathBuf::from(row.get::<_, String>(1)?),
                quarantine_root: PathBuf::from(row.get::<_, String>(2)?),
                moved: decode_json("delete_journals.moved_json", &row.get::<_, String>(3)?)?,
            })
        })?
        .collect()
    }

    pub(crate) fn download_exists(&self, id: DownloadId) -> Result<bool> {
        Ok(self.conn.lock().query_row(
            "SELECT EXISTS(SELECT 1 FROM downloads WHERE id=?1)",
            [id.0.to_string()],
            |row| row.get(0),
        )?)
    }
}

#[cfg(unix)]
fn restrict_to_owner(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    if std::fs::metadata(path)?.permissions().mode() & 0o777 != 0o600 {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn insert_download(tx: &Transaction<'_>, d: &Download) -> Result<()> {
    tx.execute(
        "INSERT INTO downloads (
            id, filename, save_path, total_size, downloaded, status,
            error_msg, created_at, started_at, completed_at,
            retry_count, max_retries, pinned, kind
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            d.id.0.to_string(),
            d.filename,
            path_to_utf8("downloads.save_path", &d.save_path)?,
            d.total_size
                .map(|size| encode_u64("downloads.total_size", size))
                .transpose()?,
            encode_u64("downloads.downloaded", d.downloaded)?,
            d.status.as_str(),
            d.error_message,
            d.created_at.to_rfc3339(),
            d.started_at.map(|t| t.to_rfc3339()),
            d.completed_at.map(|t| t.to_rfc3339()),
            d.retry_count,
            d.max_retries,
            d.pinned,
            d.kind.as_str(),
        ],
    )?;
    match &d.kind {
        DownloadKind::Http(http) => {
            crate::probe::validate_custom_headers(&http.headers)?;
            if let Some((name, _)) = http
                .headers
                .iter()
                .find(|(name, _)| is_sensitive_header(name))
            {
                return Err(invalid("http_state.headers", name));
            }
            tx.execute(
                "INSERT INTO http_state (download_id, url, headers, segments, subfolder, auto_extract)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    d.id.0.to_string(),
                    http.url,
                    encode_json("http_state.headers", &http.headers)?,
                    http.segments,
                    http.subfolder,
                    http.auto_extract,
                ],
            )?;
        },
        DownloadKind::Torrent(t) => {
            let (source_kind, source_data) = match &t.source {
                TorrentSource::Magnet(magnet) => ("magnet", magnet.as_bytes()),
                TorrentSource::File(bytes) => ("file", bytes.as_slice()),
            };
            tx.execute(
                "INSERT INTO torrent_state (
                    download_id, source_kind, source_data, info_hash, auto_extract, is_private,
                    files_json, trackers_json, uploaded, ratio, seed_elapsed_secs, metadata_bytes
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    d.id.0.to_string(),
                    source_kind,
                    source_data,
                    t.info_hash.as_slice(),
                    t.auto_extract,
                    t.is_private,
                    encode_json("torrent_state.files_json", &t.files)?,
                    encode_json("torrent_state.trackers_json", &t.trackers)?,
                    encode_u64("torrent_state.uploaded", t.stats.uploaded)?,
                    f64::from(t.stats.ratio),
                    encode_u64("torrent_state.seed_elapsed_secs", t.stats.seed_elapsed_secs)?,
                    t.metadata_bytes,
                ],
            )?;
        },
    }
    Ok(())
}

fn update_torrent_stats(tx: &Transaction<'_>, id: DownloadId, stats: &TorrentStats) -> Result<()> {
    tx.prepare_cached(
        "UPDATE torrent_state SET uploaded=?2, ratio=?3, seed_elapsed_secs=?4
         WHERE download_id=?1",
    )?
    .execute(params![
        id.0.to_string(),
        encode_u64("torrent_state.uploaded", stats.uploaded)?,
        f64::from(stats.ratio),
        encode_u64("torrent_state.seed_elapsed_secs", stats.seed_elapsed_secs)?,
    ])?;
    Ok(())
}

fn package_items(conn: &Connection, package_id: PackageId) -> Result<Vec<PackageItem>> {
    let mut stmt = conn.prepare(
        "SELECT download_id, position, part_number
         FROM package_items WHERE package_id=?1 ORDER BY position",
    )?;
    stmt.query_and_then([package_id.0.to_string()], |row| {
        Ok(PackageItem {
            download_id: DownloadId(decode_uuid(
                "package_items.download_id",
                &row.get::<_, String>(0)?,
            )?),
            position: decode_int("package_items.position", row.get(1)?)?,
            part_number: decode_int("package_items.part_number", row.get(2)?)?,
        })
    })?
    .collect()
}

fn parse_extract_state(value: &str) -> Result<PackageExtractState> {
    use PackageExtractState::{Completed, Error, Extracting, NotStarted, PasswordRequired};
    [NotStarted, Extracting, Completed, Error, PasswordRequired]
        .into_iter()
        .find(|state| state.as_str() == value)
        .ok_or_else(|| invalid("packages.extract_state", &value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChunkStatus, DownloadStatus, TorrentFile, TorrentState};

    fn sample() -> Download {
        Download::new(
            "https://example.com/file.zip".to_string(),
            PathBuf::from("/tmp"),
        )
    }

    fn torrent_sample() -> Download {
        let state = TorrentState::new(
            TorrentSource::Magnet(
                "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862".into(),
            ),
            [7; 20],
        );
        Download::with_kind(
            "t".into(),
            PathBuf::from("/tmp"),
            DownloadKind::Torrent(state),
        )
    }

    #[test]
    fn mutable_fields_and_torrent_stats_roundtrip() {
        let db = Database::in_memory().unwrap();
        let mut http = sample();
        let mut torrent = torrent_sample();
        torrent.torrent_mut().unwrap().metadata_bytes = Some(b"metadata".to_vec());
        db.insert_download(&http).unwrap();
        db.insert_download(&torrent).unwrap();

        http.status = DownloadStatus::Error;
        http.error_message = Some("connection reset".into());
        http.downloaded = 12_345;
        http.completed_at = Some(chrono::Utc::now());
        torrent.torrent_mut().unwrap().stats.uploaded = 2048;
        torrent.torrent_mut().unwrap().stats.seed_elapsed_secs = 300;
        db.update_downloads(&[http.clone(), torrent.clone()])
            .unwrap();

        let stored_http = db.get(http.id).unwrap().unwrap();
        assert_eq!(stored_http.status, DownloadStatus::Error);
        assert_eq!(
            stored_http.error_message.as_deref(),
            Some("connection reset")
        );
        assert_eq!(stored_http.downloaded, 12_345);
        assert!(stored_http.completed_at.is_some());
        let stored = db.get(torrent.id).unwrap().unwrap();
        assert_eq!(stored.torrent().unwrap().stats.uploaded, 2048);
        assert_eq!(stored.torrent().unwrap().stats.seed_elapsed_secs, 300);
        assert_eq!(
            stored.torrent().unwrap().metadata_bytes.as_deref(),
            Some(&b"metadata"[..])
        );
    }

    #[test]
    fn chunk_batches_replace_and_update_atomically() {
        let db = Database::in_memory().unwrap();
        let d = sample();
        db.insert_download(&d).unwrap();
        let chunk = |index, downloaded, status| ChunkInfo {
            index,
            start: u64::from(index) * 100,
            end: u64::from(index) * 100 + 99,
            downloaded,
            status,
        };
        db.replace_chunks(
            d.id,
            &[
                chunk(0, 0, ChunkStatus::Pending),
                chunk(1, 0, ChunkStatus::Pending),
            ],
        )
        .unwrap();
        db.update_chunks(
            d.id,
            &[
                chunk(0, 100, ChunkStatus::Completed),
                chunk(1, 50, ChunkStatus::Downloading),
            ],
        )
        .unwrap();
        assert_eq!(
            db.chunks(d.id)
                .unwrap()
                .iter()
                .map(|c| c.downloaded)
                .collect::<Vec<_>>(),
            [100, 50]
        );

        db.replace_chunks(d.id, &[chunk(0, 0, ChunkStatus::Pending)])
            .unwrap();
        assert_eq!(db.chunks(d.id).unwrap().len(), 1);

        db.delete_download(d.id).unwrap();
        assert!(db.chunks(d.id).unwrap().is_empty());
    }

    #[test]
    fn package_delete_removes_package_and_children() {
        let db = Database::in_memory().unwrap();
        let first = sample();
        let second = sample();
        let package = ArchivePackage {
            id: PackageId::new(),
            name: "archive".into(),
            save_path: PathBuf::from("/tmp"),
            auto_extract: false,
            extract_state: PackageExtractState::NotStarted,
            error_message: None,
            created_at: chrono::Utc::now(),
            completed_at: None,
            pinned: false,
            items: [&first, &second]
                .iter()
                .zip(0..)
                .map(|(d, position)| PackageItem {
                    download_id: d.id,
                    position,
                    part_number: position + 1,
                })
                .collect(),
        };
        db.insert_package(&package, &[first, second]).unwrap();
        assert_eq!(db.packages().unwrap(), std::slice::from_ref(&package));

        db.delete_package(&package).unwrap();

        assert_eq!(db.packages().unwrap(), Vec::<ArchivePackage>::new());
        assert_eq!(db.downloads().unwrap(), Vec::<Download>::new());
    }

    #[test]
    fn torrent_metadata_update_persists() {
        let db = Database::in_memory().unwrap();
        let mut d = torrent_sample();
        db.insert_download(&d).unwrap();
        d.filename = "release".into();
        d.total_size = Some(1024);
        let torrent = d.torrent_mut().unwrap();
        torrent.is_private = true;
        torrent.files = vec![TorrentFile {
            path: PathBuf::from("release/archive.zip"),
            size: 1024,
            selected: true,
        }];
        torrent.trackers = vec!["udp://tracker.example:1337/announce".to_string()];

        db.update_torrent_metadata(&d).unwrap();

        assert_eq!(db.get(d.id).unwrap().unwrap(), d);
    }

    #[test]
    fn sensitive_http_headers_are_not_persisted() {
        let db = Database::in_memory().unwrap();
        let mut d = sample();
        d.http_mut().unwrap().headers = vec![("Authorization".to_string(), "secret".to_string())];

        assert!(db.insert_download(&d).is_err());
    }

    #[test]
    fn hostile_rows_are_rejected() {
        let hostile = [
            "UPDATE http_state SET headers = 'not json'",
            "UPDATE downloads SET created_at = 'not a timestamp'",
            "UPDATE downloads SET filename = '../file.bin'",
            "UPDATE http_state SET subfolder = '../release'",
            "UPDATE torrent_state SET files_json = '[{\"path\":\"../x\",\"size\":1,\"selected\":true}]'",
        ];
        for statement in hostile {
            let db = Database::in_memory().unwrap();
            db.insert_download(&sample()).unwrap();
            db.insert_download(&torrent_sample()).unwrap();
            db.conn.lock().execute(statement, []).unwrap();

            assert!(db.downloads().is_err(), "{statement}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn database_and_sidecars_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.db");
        std::fs::write(&path, b"").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        let db = Database::open(&path).unwrap();
        db.insert_download(&sample()).unwrap();

        for suffix in ["", "-wal", "-shm"] {
            let candidate = PathBuf::from(format!("{}{suffix}", path.display()));
            if candidate.exists() {
                let mode = std::fs::metadata(&candidate).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o600, "{}", candidate.display());
            }
        }
    }
}
