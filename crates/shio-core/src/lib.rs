mod config;
mod db;
mod engine;
mod error;
mod extract;
mod filename;
mod format;
mod path;
mod torrent;
mod types;

pub(crate) mod chunk;
pub(crate) mod probe;
pub(crate) mod queue;
pub(crate) mod worker;

pub use config::{
    AppConfig, ThemeConfig, WindowConfig, WindowMaterialPreference, validate_theme_id,
};
pub use engine::{
    DownloadEngine, EngineCommand, HttpArchivePartRequest, HttpArchiveSetRequest,
    HttpDownloadRequest, ProgressStream, TorrentDownloadRequest,
};
pub use error::{Result, ShioError};
pub use filename::{
    ArchivePart, extract_filename, is_archive_filename, parse_archive_part, sanitize_filename,
    subfolder_value, suggest_folder_name,
};
pub use format::{format_eta, format_speed};
pub use torrent::{SeedPolicy, TorrentManifest, parse_torrent_manifest};
pub use types::{
    ArchivePackage, ChunkInfo, ChunkStatus, Download, DownloadId, DownloadKind, DownloadProgress,
    DownloadStatus, HttpPreview, HttpPreviewResult, HttpPreviewState, HttpState,
    MagnetPreviewManifest, MagnetPreviewResult, PackageExtractState, PackageId, PackageItem,
    PackageKind, ProgressDetail, ProxyConfig, ProxyProtocol, TorrentFile, TorrentProgressSnapshot,
    TorrentSource, TorrentState,
};
