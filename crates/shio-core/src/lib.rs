mod chunk;
mod config;
mod db;
mod engine;
mod error;
mod extract;
mod filename;
mod path;
mod probe;
mod resolve;
mod torrent;
mod types;
mod worker;

pub use chunk::MAX_SEGMENTS;
pub use config::{AppConfig, ThemeConfig, TorrentConfig, WindowConfig, validate_theme_id};
pub use engine::{
    DownloadEngine, EngineCommand, HttpArchivePartRequest, HttpArchiveSetRequest,
    HttpDownloadRequest, ProgressEvent, ProgressStream, TorrentDownloadRequest,
};
pub use error::{Result, ShioError};
pub use filename::{
    ArchivePart, extract_filename, parse_archive_part, sanitize_filename, subfolder_value,
    suggest_folder_name,
};
pub use torrent::{
    MAX_TORRENT_METADATA_BYTES, SeedPolicy, TorrentManifest, parse_torrent_manifest,
};
pub use types::{
    ArchivePackage, Download, DownloadId, DownloadKind, DownloadProgress, DownloadStatus,
    HttpPreview, HttpPreviewResult, HttpPreviewState, HttpState, MagnetPreviewManifest,
    MagnetPreviewResult, PackageExtractState, PackageId, PackageItem, ProxyConfig, ProxyProtocol,
    TorrentFile, TorrentSource, TorrentState, TorrentStats,
};
