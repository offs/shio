use rusqlite::Connection;

use crate::error::{Result, ShioError};

const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE downloads (
    id           TEXT PRIMARY KEY,
    filename     TEXT NOT NULL,
    save_path    TEXT NOT NULL,
    total_size   INTEGER CHECK (total_size IS NULL OR total_size >= 0),
    downloaded   INTEGER NOT NULL CHECK (downloaded >= 0),
    status       TEXT NOT NULL CHECK (
        status IN (
            'pending', 'queued', 'starting', 'downloading', 'fetching_metadata',
            'extracting', 'paused', 'seeding', 'completed', 'error',
            'extract_error', 'password_required', 'cancelled'
        )
    ),
    error_msg    TEXT,
    created_at   TEXT NOT NULL,
    started_at   TEXT,
    completed_at TEXT,
    retry_count  INTEGER NOT NULL CHECK (retry_count >= 0),
    max_retries  INTEGER NOT NULL CHECK (max_retries >= 0),
    pinned       INTEGER NOT NULL CHECK (pinned IN (0, 1)),
    kind         TEXT NOT NULL CHECK (kind IN ('http', 'torrent'))
);

CREATE TABLE http_state (
    download_id  TEXT PRIMARY KEY REFERENCES downloads(id) ON DELETE CASCADE,
    url          TEXT NOT NULL,
    headers      TEXT NOT NULL,
    segments     INTEGER NOT NULL CHECK (segments >= 1 AND segments <= 16),
    subfolder    TEXT,
    auto_extract INTEGER NOT NULL CHECK (auto_extract IN (0, 1))
);

CREATE TABLE torrent_state (
    download_id  TEXT PRIMARY KEY REFERENCES downloads(id) ON DELETE CASCADE,
    source_kind  TEXT NOT NULL CHECK (source_kind IN ('magnet', 'file')),
    source_data  BLOB NOT NULL,
    info_hash    BLOB NOT NULL CHECK (length(info_hash) = 20),
    auto_extract INTEGER NOT NULL CHECK (auto_extract IN (0, 1)),
    is_private   INTEGER NOT NULL CHECK (is_private IN (0, 1)),
    files_json   TEXT NOT NULL,
    trackers_json TEXT NOT NULL,
    uploaded     INTEGER NOT NULL CHECK (uploaded >= 0),
    ratio        REAL NOT NULL CHECK (ratio >= 0),
    seed_elapsed_secs INTEGER NOT NULL CHECK (seed_elapsed_secs >= 0),
    metadata_bytes BLOB
);

CREATE TABLE chunks (
    download_id TEXT NOT NULL REFERENCES downloads(id) ON DELETE CASCADE,
    chunk_index INTEGER NOT NULL CHECK (chunk_index >= 0),
    start_byte  INTEGER NOT NULL CHECK (start_byte >= 0),
    end_byte    INTEGER NOT NULL CHECK (end_byte >= 0),
    downloaded  INTEGER NOT NULL CHECK (downloaded >= 0),
    status      TEXT NOT NULL CHECK (status IN ('pending', 'downloading', 'completed')),
    PRIMARY KEY (download_id, chunk_index)
);

CREATE TABLE packages (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    save_path    TEXT NOT NULL,
    auto_extract INTEGER NOT NULL CHECK (auto_extract IN (0, 1)),
    extract_state TEXT NOT NULL CHECK (
        extract_state IN (
            'not_started', 'extracting', 'completed', 'error', 'password_required'
        )
    ),
    error_msg    TEXT,
    created_at   TEXT NOT NULL,
    completed_at TEXT,
    pinned       INTEGER NOT NULL CHECK (pinned IN (0, 1))
);

CREATE TABLE package_items (
    package_id   TEXT NOT NULL REFERENCES packages(id) ON DELETE CASCADE,
    download_id  TEXT NOT NULL REFERENCES downloads(id) ON DELETE CASCADE,
    position     INTEGER NOT NULL CHECK (position >= 0),
    part_number  INTEGER NOT NULL CHECK (part_number >= 1),
    PRIMARY KEY (package_id, download_id),
    UNIQUE(package_id, position),
    UNIQUE(package_id, part_number)
);

CREATE TABLE delete_journals (
    download_id    TEXT PRIMARY KEY,
    protected_root TEXT NOT NULL,
    quarantine_root TEXT NOT NULL,
    moved_json     TEXT NOT NULL
);

CREATE INDEX idx_downloads_created ON downloads(created_at);
CREATE INDEX idx_package_items_download ON package_items(download_id);
";

const APPLICATION_ID: i64 = 0x5348_494F;

// Creation and stamps share one transaction, so any other combination is not Shio's database.
pub(super) fn ensure_schema(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let application: i64 = tx.query_row("PRAGMA application_id", [], |row| row.get(0))?;
    let found: i64 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let objects: i64 = tx.query_row(
        "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )?;
    match (application, found, objects) {
        (APPLICATION_ID, SCHEMA_VERSION, _) => return Ok(()),
        (0, 0, 0) => tx.execute_batch(SCHEMA)?,
        _ => return Err(ShioError::UnrecognizedDatabase),
    }
    tx.pragma_update(None, "application_id", APPLICATION_ID)?;
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_file_is_created_once_and_foreign_files_are_refused() {
        let mut fresh = Connection::open_in_memory().unwrap();
        ensure_schema(&mut fresh).unwrap();
        ensure_schema(&mut fresh).unwrap();

        for foreign in [
            "CREATE TABLE notes (body TEXT);",
            "CREATE TABLE downloads (id TEXT); PRAGMA user_version = 1;",
        ] {
            let mut conn = Connection::open_in_memory().unwrap();
            conn.execute_batch(foreign).unwrap();
            assert!(ensure_schema(&mut conn).is_err(), "{foreign}");
        }
    }
}
