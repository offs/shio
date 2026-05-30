use rusqlite::Connection;

use crate::error::{Result, ShioError};

pub(super) const SCHEMA_VERSION: i64 = 1;

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS downloads (
    id           TEXT PRIMARY KEY,
    filename     TEXT NOT NULL,
    save_path    TEXT NOT NULL,
    total_size   INTEGER CHECK (total_size IS NULL OR total_size >= 0),
    downloaded   INTEGER NOT NULL DEFAULT 0 CHECK (downloaded >= 0),
    status       TEXT NOT NULL DEFAULT 'pending' CHECK (
        status IN (
            'pending', 'queued', 'starting', 'downloading', 'fetching_metadata',
            'extracting', 'paused', 'seeding', 'completed', 'error',
            'extract_error', 'password_required', 'cancelled'
        )
    ),
    priority     INTEGER NOT NULL DEFAULT 0,
    error_msg    TEXT,
    created_at   TEXT NOT NULL,
    started_at   TEXT,
    completed_at TEXT,
    retry_count  INTEGER NOT NULL DEFAULT 0 CHECK (retry_count >= 0),
    max_retries  INTEGER NOT NULL DEFAULT 3 CHECK (max_retries >= 0),
    pinned       INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1)),
    kind         TEXT NOT NULL DEFAULT 'http' CHECK (kind IN ('http', 'torrent'))
);

CREATE TABLE IF NOT EXISTS http_state (
    download_id  TEXT PRIMARY KEY REFERENCES downloads(id) ON DELETE CASCADE,
    url          TEXT NOT NULL,
    headers      TEXT NOT NULL DEFAULT '[]',
    segments     INTEGER NOT NULL DEFAULT 8 CHECK (segments >= 1 AND segments <= 255),
    subfolder    TEXT,
    auto_extract INTEGER NOT NULL DEFAULT 0 CHECK (auto_extract IN (0, 1))
);

CREATE TABLE IF NOT EXISTS torrent_state (
    download_id  TEXT PRIMARY KEY REFERENCES downloads(id) ON DELETE CASCADE,
    source_kind  TEXT NOT NULL CHECK (source_kind IN ('magnet', 'file')),
    source_data  BLOB NOT NULL,
    info_hash    BLOB NOT NULL CHECK (length(info_hash) = 20),
    auto_extract INTEGER NOT NULL DEFAULT 0 CHECK (auto_extract IN (0, 1)),
    is_private   INTEGER NOT NULL DEFAULT 0 CHECK (is_private IN (0, 1)),
    files_json   TEXT NOT NULL DEFAULT '[]',
    trackers_json TEXT NOT NULL DEFAULT '[]',
    uploaded     INTEGER NOT NULL DEFAULT 0 CHECK (uploaded >= 0),
    ratio        REAL NOT NULL DEFAULT 0 CHECK (ratio >= 0),
    seed_elapsed_secs INTEGER NOT NULL DEFAULT 0 CHECK (seed_elapsed_secs >= 0),
    metadata_bytes BLOB
);

CREATE TABLE IF NOT EXISTS chunks (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    download_id TEXT NOT NULL REFERENCES downloads(id) ON DELETE CASCADE,
    chunk_index INTEGER NOT NULL CHECK (chunk_index >= 0),
    start_byte  INTEGER NOT NULL CHECK (start_byte >= 0),
    end_byte    INTEGER NOT NULL CHECK (end_byte >= 0),
    downloaded  INTEGER NOT NULL DEFAULT 0 CHECK (downloaded >= 0),
    status      TEXT NOT NULL DEFAULT 'pending' CHECK (
        status IN ('pending', 'downloading', 'completed', 'error')
    ),
    UNIQUE(download_id, chunk_index)
);

CREATE TABLE IF NOT EXISTS packages (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    save_path    TEXT NOT NULL,
    kind         TEXT NOT NULL CHECK (kind = 'archive_set'),
    auto_extract INTEGER NOT NULL DEFAULT 0 CHECK (auto_extract IN (0, 1)),
    extract_state TEXT NOT NULL DEFAULT 'not_started' CHECK (
        extract_state IN (
            'not_started', 'extracting', 'completed', 'error', 'password_required'
        )
    ),
    error_msg    TEXT,
    created_at   TEXT NOT NULL,
    completed_at TEXT,
    pinned       INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1))
);

CREATE TABLE IF NOT EXISTS package_items (
    package_id   TEXT NOT NULL REFERENCES packages(id) ON DELETE CASCADE,
    download_id  TEXT NOT NULL REFERENCES downloads(id) ON DELETE CASCADE,
    position     INTEGER NOT NULL CHECK (position >= 0),
    part_number  INTEGER NOT NULL CHECK (part_number >= 1),
    PRIMARY KEY (package_id, download_id),
    UNIQUE(package_id, position),
    UNIQUE(package_id, part_number)
);

CREATE INDEX IF NOT EXISTS idx_downloads_status ON downloads(status);
CREATE INDEX IF NOT EXISTS idx_downloads_created ON downloads(created_at);
CREATE INDEX IF NOT EXISTS idx_chunks_download ON chunks(download_id);
CREATE INDEX IF NOT EXISTS idx_torrent_state_info_hash ON torrent_state(info_hash);
CREATE INDEX IF NOT EXISTS idx_package_items_download ON package_items(download_id);
";

pub(super) fn require_current_schema(conn: &Connection, allow_empty_schema: bool) -> Result<()> {
    let found = schema_version(conn)?;
    if found == SCHEMA_VERSION || (allow_empty_schema && found == 0) {
        return Ok(());
    }
    Err(ShioError::UnsupportedDatabaseVersion {
        found,
        expected: SCHEMA_VERSION,
    })
}

pub(super) fn set_schema_version(conn: &Connection, version: i64) -> Result<()> {
    conn.pragma_update(None, "user_version", version)?;
    Ok(())
}

fn schema_version(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("PRAGMA user_version", [], |row| row.get(0))?)
}
