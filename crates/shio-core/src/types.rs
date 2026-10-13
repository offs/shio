use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DownloadId(pub Uuid);

impl DownloadId {
    pub(crate) fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for DownloadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PackageId(pub Uuid);

impl PackageId {
    pub(crate) fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub const fn row_id(self) -> DownloadId {
        DownloadId(self.0)
    }
}

impl fmt::Display for PackageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadStatus {
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
}

impl DownloadStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Queued => "queued",
            Self::Starting => "starting",
            Self::Downloading => "downloading",
            Self::FetchingMetadata => "fetching_metadata",
            Self::Extracting => "extracting",
            Self::Paused => "paused",
            Self::Seeding => "seeding",
            Self::Completed => "completed",
            Self::Error => "error",
            Self::ExtractError => "extract_error",
            Self::PasswordRequired => "password_required",
            Self::Cancelled => "cancelled",
        }
    }

    pub const fn is_active(self) -> bool {
        matches!(
            self,
            Self::Starting
                | Self::Downloading
                | Self::Extracting
                | Self::FetchingMetadata
                | Self::Seeding
        )
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }

    pub const fn is_failed(self) -> bool {
        matches!(
            self,
            Self::Error | Self::ExtractError | Self::PasswordRequired
        )
    }

    pub(crate) const fn is_transferring(self) -> bool {
        matches!(
            self,
            Self::Starting | Self::Downloading | Self::FetchingMetadata
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Download {
    pub id: DownloadId,
    pub filename: String,
    pub save_path: PathBuf,
    pub total_size: Option<u64>,
    pub downloaded: u64,
    pub status: DownloadStatus,
    pub speed: u64,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub retry_count: u32,
    pub max_retries: u32,
    pub pinned: bool,
    pub kind: DownloadKind,
}

impl Download {
    #[cfg(test)]
    pub(crate) fn new(url: String, save_path: PathBuf) -> Self {
        Self::with_kind(
            crate::filename::extract_filename(&url),
            save_path,
            DownloadKind::Http(HttpState::new(url)),
        )
    }

    pub(crate) fn with_kind(filename: String, save_path: PathBuf, kind: DownloadKind) -> Self {
        Self {
            id: DownloadId::new(),
            filename,
            save_path,
            total_size: None,
            downloaded: 0,
            status: DownloadStatus::Pending,
            speed: 0,
            error_message: None,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            retry_count: 0,
            max_retries: 0,
            pinned: false,
            kind,
        }
    }

    pub const fn http(&self) -> Option<&HttpState> {
        match &self.kind {
            DownloadKind::Http(http) => Some(http),
            DownloadKind::Torrent(_) => None,
        }
    }

    pub const fn torrent(&self) -> Option<&TorrentState> {
        match &self.kind {
            DownloadKind::Torrent(torrent) => Some(torrent),
            DownloadKind::Http(_) => None,
        }
    }

    #[cfg(test)]
    pub(crate) const fn http_mut(&mut self) -> Option<&mut HttpState> {
        match &mut self.kind {
            DownloadKind::Http(http) => Some(http),
            DownloadKind::Torrent(_) => None,
        }
    }

    pub(crate) const fn torrent_mut(&mut self) -> Option<&mut TorrentState> {
        match &mut self.kind {
            DownloadKind::Torrent(torrent) => Some(torrent),
            DownloadKind::Http(_) => None,
        }
    }

    pub fn file_path(&self) -> PathBuf {
        if let Some(path) = self.torrent_single_file_path() {
            return self.save_path.join(path);
        }
        self.file_dir().join(&self.filename)
    }

    pub fn file_dir(&self) -> PathBuf {
        if let Some(path) = self.torrent_single_file_path() {
            return path.parent().map_or_else(
                || self.save_path.clone(),
                |parent| self.save_path.join(parent),
            );
        }
        if let Some(root) = self.torrent_common_root() {
            return self.save_path.join(root);
        }
        match self.http().and_then(|http| http.subfolder.as_deref()) {
            Some(subfolder) => self.save_path.join(subfolder),
            None => self.save_path.clone(),
        }
    }

    fn torrent_files(&self) -> impl Iterator<Item = &TorrentFile> {
        let files = self.torrent().map_or(&[][..], |torrent| &torrent.files);
        let any_selected = files.iter().any(|file| file.selected);
        files
            .iter()
            .filter(move |file| file.selected || !any_selected)
    }

    fn torrent_single_file_path(&self) -> Option<&PathBuf> {
        let mut files = self.torrent_files();
        let file = files.next()?;
        files.next().is_none().then_some(&file.path)
    }

    fn torrent_common_root(&self) -> Option<PathBuf> {
        let mut paths = self
            .torrent_files()
            .map(|file| file.path.components().collect::<Vec<_>>());
        let first = paths.next()?;
        let shared = paths.fold(first, |shared, parts| {
            shared
                .into_iter()
                .zip(parts)
                .take_while(|(left, right)| left == right)
                .map(|(component, _)| component)
                .collect()
        });
        (!shared.is_empty()).then(|| shared.iter().collect())
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct HttpState {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub segments: u8,
    pub subfolder: Option<String>,
    pub auto_extract: bool,
}

impl HttpState {
    pub(crate) const fn new(url: String) -> Self {
        Self {
            url,
            headers: Vec::new(),
            segments: 8,
            subfolder: None,
            auto_extract: false,
        }
    }
}

impl fmt::Debug for HttpState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let headers: Vec<_> = self
            .headers
            .iter()
            .map(|(name, value)| {
                let value = if is_sensitive_header(name) {
                    "<redacted>"
                } else {
                    value.as_str()
                };
                (name.as_str(), value)
            })
            .collect();
        f.debug_struct("HttpState")
            .field("url", &"<redacted>")
            .field("headers", &headers)
            .field("segments", &self.segments)
            .field("subfolder", &self.subfolder)
            .field("auto_extract", &self.auto_extract)
            .finish()
    }
}

pub(crate) fn is_sensitive_header(name: &str) -> bool {
    [
        "authorization",
        "cookie",
        "proxy-authorization",
        "set-cookie",
    ]
    .iter()
    .any(|sensitive| name.eq_ignore_ascii_case(sensitive))
}

#[derive(Clone, PartialEq, Eq)]
pub enum TorrentSource {
    Magnet(String),
    File(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentFile {
    pub path: PathBuf,
    pub size: u64,
    pub selected: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TorrentStats {
    pub peers: u32,
    pub uploaded: u64,
    pub upload_speed: u64,
    pub ratio: f32,
    pub seed_elapsed_secs: u64,
    pub metadata_wait_secs: u64,
}

#[derive(Clone, PartialEq)]
pub struct TorrentState {
    pub source: TorrentSource,
    pub info_hash: [u8; 20],
    pub metadata_bytes: Option<Vec<u8>>,
    pub auto_extract: bool,
    pub is_private: bool,
    pub files: Vec<TorrentFile>,
    pub trackers: Vec<String>,
    pub stats: TorrentStats,
}

impl TorrentState {
    pub(crate) fn new(source: TorrentSource, info_hash: [u8; 20]) -> Self {
        Self {
            source,
            info_hash,
            metadata_bytes: None,
            auto_extract: false,
            is_private: false,
            files: Vec::new(),
            trackers: Vec::new(),
            stats: TorrentStats::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DownloadKind {
    Http(HttpState),
    Torrent(TorrentState),
}

impl DownloadKind {
    pub(crate) const fn as_str(&self) -> &'static str {
        match self {
            Self::Http(_) => "http",
            Self::Torrent(_) => "torrent",
        }
    }

    pub const fn is_http(&self) -> bool {
        matches!(self, Self::Http(_))
    }

    pub const fn is_torrent(&self) -> bool {
        matches!(self, Self::Torrent(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageExtractState {
    NotStarted,
    Extracting,
    Completed,
    Error,
    PasswordRequired,
}

impl PackageExtractState {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::NotStarted => "not_started",
            Self::Extracting => "extracting",
            Self::Completed => "completed",
            Self::Error => "error",
            Self::PasswordRequired => "password_required",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageItem {
    pub download_id: DownloadId,
    pub position: u32,
    pub part_number: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivePackage {
    pub id: PackageId,
    pub name: String,
    pub save_path: PathBuf,
    pub auto_extract: bool,
    pub extract_state: PackageExtractState,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub pinned: bool,
    pub items: Vec<PackageItem>,
}

impl ArchivePackage {
    pub fn folder_path(&self) -> PathBuf {
        self.save_path.join(&self.name)
    }

    pub fn child_ids(&self) -> impl Iterator<Item = DownloadId> + '_ {
        self.items.iter().map(|item| item.download_id)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct MagnetPreviewManifest {
    pub name: String,
    pub total_size: u64,
    pub is_private: bool,
    pub files: Vec<TorrentFile>,
    pub trackers: Vec<String>,
    pub metadata_bytes: Vec<u8>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct MagnetPreviewResult {
    pub request_id: u64,
    pub magnet: String,
    pub result: std::result::Result<MagnetPreviewManifest, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpPreview {
    pub filename: String,
    pub total_size: Option<u64>,
    pub content_type: Option<String>,
    pub accept_ranges: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpPreviewState {
    Ready(HttpPreview),
    Blocked { reason: String },
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpPreviewResult {
    pub request_id: u64,
    pub state: HttpPreviewState,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DownloadProgress {
    pub id: DownloadId,
    pub status: DownloadStatus,
    pub downloaded: u64,
    pub total_size: Option<u64>,
    pub speed: u64,
    pub torrent: Option<TorrentStats>,
}

#[derive(Debug, Clone)]
pub(crate) struct ChunkInfo {
    pub(crate) index: u32,
    pub(crate) start: u64,
    pub(crate) end: u64,
    pub(crate) downloaded: u64,
    pub(crate) status: ChunkStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChunkStatus {
    Pending,
    Downloading,
    Completed,
}

impl ChunkStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Downloading => "downloading",
            Self::Completed => "completed",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyConfig {
    pub protocol: ProxyProtocol,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl fmt::Debug for ProxyConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyConfig")
            .field("protocol", &self.protocol)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username.as_ref().map(|_| "<redacted>"))
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProxyProtocol {
    Http,
    Socks5,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn torrent(files: &[(&str, bool)]) -> Download {
        let mut state = TorrentState::new(TorrentSource::Magnet(String::new()), [0; 20]);
        state.files = files
            .iter()
            .map(|(path, selected)| TorrentFile {
                path: PathBuf::from(path),
                size: 1,
                selected: *selected,
            })
            .collect();
        Download::with_kind(
            "t".into(),
            PathBuf::from("/root"),
            DownloadKind::Torrent(state),
        )
    }

    #[test]
    fn torrent_payload_location_follows_selected_files() {
        let single = torrent(&[("folder/a.zip", true), ("folder/b.zip", false)]);
        assert_eq!(single.file_path(), PathBuf::from("/root/folder/a.zip"));
        assert_eq!(single.file_dir(), PathBuf::from("/root/folder"));

        let shared = torrent(&[("release/cd1.rar", true), ("release/cd2.rar", true)]);
        assert_eq!(shared.file_dir(), PathBuf::from("/root/release"));
    }
}
