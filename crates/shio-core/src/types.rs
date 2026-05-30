use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DownloadId(pub Uuid);

impl DownloadId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for DownloadId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for DownloadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PackageId(pub Uuid);

impl PackageId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for PackageId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for PackageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    pub const fn as_str(self) -> &'static str {
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

    pub const fn is_seeding(self) -> bool {
        matches!(self, Self::Seeding)
    }
}

impl fmt::Display for DownloadStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Download {
    pub id: DownloadId,
    pub filename: String,
    pub save_path: PathBuf,
    pub total_size: Option<u64>,
    pub downloaded: u64,
    pub status: DownloadStatus,
    pub priority: i32,
    pub speed: u64,
    #[serde(skip)]
    pub avg_speed: u64,
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
    pub fn new(url: String, save_path: PathBuf) -> Self {
        let filename = crate::filename::extract_filename(&url, None);
        Self {
            id: DownloadId::new(),
            filename,
            save_path,
            total_size: None,
            downloaded: 0,
            status: DownloadStatus::Pending,
            priority: 0,
            speed: 0,
            avg_speed: 0,
            error_message: None,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            retry_count: 0,
            max_retries: 3,
            pinned: false,
            kind: DownloadKind::Http(HttpState::new(url)),
        }
    }

    pub const fn http(&self) -> Option<&HttpState> {
        self.kind.as_http()
    }

    pub const fn http_mut(&mut self) -> Option<&mut HttpState> {
        self.kind.as_http_mut()
    }

    pub const fn torrent(&self) -> Option<&TorrentState> {
        self.kind.as_torrent()
    }

    pub const fn torrent_mut(&mut self) -> Option<&mut TorrentState> {
        self.kind.as_torrent_mut()
    }

    pub fn url(&self) -> Option<&str> {
        self.http().map(|s| s.url.as_str())
    }

    pub fn progress_percent(&self) -> f32 {
        match self.total_size {
            Some(total) if total > 0 => (self.downloaded as f32 / total as f32) * 100.0,
            _ => 0.0,
        }
    }

    pub fn eta_seconds(&self) -> Option<u64> {
        let total = self.total_size?;
        let reference_speed = if self.avg_speed > 0 {
            self.avg_speed
        } else {
            self.speed
        };
        (reference_speed > 0).then(|| total.saturating_sub(self.downloaded) / reference_speed)
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
        match self.http().and_then(|s| s.subfolder.as_deref()) {
            Some(s) if !s.is_empty() => self.save_path.join(s),
            _ => self.save_path.clone(),
        }
    }

    pub fn try_from_torrent(source: TorrentSource, save_path: PathBuf) -> crate::Result<Self> {
        let manifest = match &source {
            TorrentSource::Magnet(_) => None,
            TorrentSource::File(bytes) => Some(crate::torrent::parse_torrent_manifest(bytes)?),
        };
        let info_hash = match (&source, &manifest) {
            (TorrentSource::Magnet(m), None) => crate::torrent::parse_magnet_info_hash(m)?,
            (TorrentSource::File(_), Some(manifest)) => manifest.info_hash,
            (TorrentSource::Magnet(_), Some(_)) | (TorrentSource::File(_), None) => {
                return Err(crate::ShioError::Other(
                    "invalid torrent source state".into(),
                ));
            },
        };
        let filename = manifest.as_ref().map_or_else(
            || "Fetching metadata\u{2026}".to_string(),
            |m| m.name.clone(),
        );
        let total_size = manifest.as_ref().map(|m| m.total_size);
        let mut state = TorrentState::new(source, info_hash);
        if let Some(manifest) = manifest {
            state.is_private = manifest.is_private;
            state.files = manifest.files;
            state.trackers = manifest.trackers;
        }
        Ok(Self {
            id: DownloadId::new(),
            filename,
            save_path,
            total_size,
            downloaded: 0,
            status: DownloadStatus::Pending,
            priority: 0,
            speed: 0,
            avg_speed: 0,
            error_message: None,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            retry_count: 0,
            max_retries: 0,
            pinned: false,
            kind: DownloadKind::Torrent(state),
        })
    }

    fn torrent_selected_files(&self) -> Vec<&TorrentFile> {
        let Some(torrent) = self.torrent() else {
            return Vec::new();
        };
        let selected: Vec<_> = torrent.files.iter().filter(|file| file.selected).collect();
        if selected.is_empty() {
            torrent.files.iter().collect()
        } else {
            selected
        }
    }

    fn torrent_single_file_path(&self) -> Option<&PathBuf> {
        let files = self.torrent_selected_files();
        if files.len() == 1 {
            Some(&files[0].path)
        } else {
            None
        }
    }

    fn torrent_common_root(&self) -> Option<PathBuf> {
        let files = self.torrent_selected_files();
        let mut components = files
            .into_iter()
            .map(|file| file.path.components().collect::<Vec<_>>());
        let first = components.next()?;
        let shared = components.fold(first, |shared, parts| {
            shared
                .into_iter()
                .zip(parts)
                .take_while(|(left, right)| left == right)
                .map(|(component, _)| component)
                .collect()
        });
        if shared.is_empty() {
            return None;
        }
        let mut out = PathBuf::new();
        for component in shared {
            out.push(component.as_os_str());
        }
        Some(out)
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpState {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub segments: u8,
    pub subfolder: Option<String>,
    pub auto_extract: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PackageKind {
    ArchiveSet,
}

impl PackageKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ArchiveSet => "archive_set",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PackageExtractState {
    NotStarted,
    Extracting,
    Completed,
    Error,
    PasswordRequired,
}

impl PackageExtractState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotStarted => "not_started",
            Self::Extracting => "extracting",
            Self::Completed => "completed",
            Self::Error => "error",
            Self::PasswordRequired => "password_required",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageItem {
    pub download_id: DownloadId,
    pub position: u32,
    pub part_number: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchivePackage {
    pub id: PackageId,
    pub name: String,
    pub save_path: PathBuf,
    pub kind: PackageKind,
    pub auto_extract: bool,
    pub extract_state: PackageExtractState,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub pinned: bool,
    pub items: Vec<PackageItem>,
}

impl ArchivePackage {
    pub fn new(name: String, save_path: PathBuf, auto_extract: bool) -> Self {
        Self {
            id: PackageId::new(),
            name,
            save_path,
            kind: PackageKind::ArchiveSet,
            auto_extract,
            extract_state: PackageExtractState::NotStarted,
            error_message: None,
            created_at: Utc::now(),
            completed_at: None,
            pinned: false,
            items: Vec::new(),
        }
    }

    pub fn folder_path(&self) -> PathBuf {
        self.save_path.join(&self.name)
    }

    pub fn child_ids(&self) -> impl Iterator<Item = DownloadId> + '_ {
        self.items.iter().map(|item| item.download_id)
    }
}

impl fmt::Debug for HttpState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpState")
            .field("url", &"<redacted>")
            .field("headers", &redacted_headers(&self.headers))
            .field("segments", &self.segments)
            .field("subfolder", &self.subfolder)
            .field("auto_extract", &self.auto_extract)
            .finish()
    }
}

impl HttpState {
    pub const fn new(url: String) -> Self {
        Self {
            url,
            headers: Vec::new(),
            segments: 8,
            subfolder: None,
            auto_extract: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "src", content = "data", rename_all = "lowercase")]
pub enum TorrentSource {
    Magnet(String),
    File(Vec<u8>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TorrentFile {
    pub path: PathBuf,
    pub size: u64,
    pub downloaded: u64,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MagnetPreviewManifest {
    pub name: String,
    pub total_size: u64,
    pub is_private: bool,
    pub files: Vec<TorrentFile>,
    pub trackers: Vec<String>,
    pub metadata_bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TorrentProgressSnapshot {
    pub is_private: bool,
    pub files: Vec<TorrentFile>,
    pub trackers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TorrentState {
    pub source: TorrentSource,
    #[serde(with = "hex_array_20")]
    pub info_hash: [u8; 20],
    pub metadata_bytes: Option<Vec<u8>>,
    pub auto_extract: bool,
    pub is_private: bool,
    pub files: Vec<TorrentFile>,
    pub trackers: Vec<String>,

    pub peers_connected: u32,
    pub seeders: u32,
    pub leechers: u32,
    pub uploaded: u64,
    pub upload_speed: u64,
    pub seed_elapsed_secs: u64,
    pub ratio: f32,
    pub librqbit_id: Option<usize>,
    pub metadata_wait_secs: u64,
}

impl TorrentState {
    pub const fn new(source: TorrentSource, info_hash: [u8; 20]) -> Self {
        Self {
            source,
            info_hash,
            metadata_bytes: None,
            auto_extract: false,
            is_private: false,
            files: Vec::new(),
            trackers: Vec::new(),
            peers_connected: 0,
            seeders: 0,
            leechers: 0,
            uploaded: 0,
            upload_speed: 0,
            seed_elapsed_secs: 0,
            ratio: 0.0,
            librqbit_id: None,
            metadata_wait_secs: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum DownloadKind {
    Http(HttpState),
    Torrent(TorrentState),
}

impl DownloadKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Http(_) => "http",
            Self::Torrent(_) => "torrent",
        }
    }

    pub const fn as_http(&self) -> Option<&HttpState> {
        match self {
            Self::Http(s) => Some(s),
            Self::Torrent(_) => None,
        }
    }

    pub const fn as_http_mut(&mut self) -> Option<&mut HttpState> {
        match self {
            Self::Http(s) => Some(s),
            Self::Torrent(_) => None,
        }
    }

    pub const fn as_torrent(&self) -> Option<&TorrentState> {
        match self {
            Self::Torrent(s) => Some(s),
            Self::Http(_) => None,
        }
    }

    pub const fn as_torrent_mut(&mut self) -> Option<&mut TorrentState> {
        match self {
            Self::Torrent(s) => Some(s),
            Self::Http(_) => None,
        }
    }

    pub const fn is_http(&self) -> bool {
        matches!(self, Self::Http(_))
    }

    pub const fn is_torrent(&self) -> bool {
        matches!(self, Self::Torrent(_))
    }
}

#[derive(Debug, Clone)]
pub enum ProgressDetail {
    Http {
        chunks: Vec<ChunkInfo>,
    },
    Torrent {
        peers_connected: u32,
        seeders: u32,
        leechers: u32,
        uploaded: u64,
        upload_speed: u64,
        ratio: f32,
        seed_elapsed_secs: u64,
        metadata_wait_secs: u64,
    },
}

impl ProgressDetail {
    pub const fn http_chunks(&self) -> Option<&[ChunkInfo]> {
        match self {
            Self::Http { chunks } => Some(chunks.as_slice()),
            Self::Torrent { .. } => None,
        }
    }

    pub const fn empty_http() -> Self {
        Self::Http { chunks: Vec::new() }
    }

    pub const fn empty_torrent() -> Self {
        Self::Torrent {
            peers_connected: 0,
            seeders: 0,
            leechers: 0,
            uploaded: 0,
            upload_speed: 0,
            ratio: 0.0,
            seed_elapsed_secs: 0,
            metadata_wait_secs: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkInfo {
    pub index: u32,
    pub start: u64,
    pub end: u64,
    pub downloaded: u64,
    pub status: ChunkStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChunkStatus {
    Pending,
    Downloading,
    Completed,
    Error,
}

impl ChunkStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Downloading => "downloading",
            Self::Completed => "completed",
            Self::Error => "error",
        }
    }
}

impl fmt::Display for ChunkStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct DownloadProgress {
    pub id: DownloadId,
    pub downloaded: u64,
    pub total_size: Option<u64>,
    pub speed: u64,
    pub avg_speed: u64,
    pub status: DownloadStatus,
    pub detail: ProgressDetail,
    pub filename: Option<String>,
    pub torrent_snapshot: Option<TorrentProgressSnapshot>,
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

fn redacted_headers(headers: &[(String, String)]) -> Vec<(&str, &str)> {
    headers
        .iter()
        .map(|(name, value)| {
            if is_sensitive_header(name) {
                (name.as_str(), "<redacted>")
            } else {
                (name.as_str(), value.as_str())
            }
        })
        .collect()
}

pub(crate) fn is_sensitive_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization" | "cookie" | "proxy-authorization" | "set-cookie"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProxyProtocol {
    Http,
    Socks5,
}

mod hex_array_20 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(bytes: &[u8; 20], ser: S) -> Result<S::Ok, S::Error> {
        use std::fmt::Write;
        let mut hex = String::with_capacity(40);
        for b in bytes {
            let _ = write!(hex, "{b:02x}");
        }
        ser.serialize_str(&hex)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<[u8; 20], D::Error> {
        let s = String::deserialize(de)?;
        if s.len() != 40 {
            return Err(serde::de::Error::custom("expected 40-char hex"));
        }
        let mut out = [0u8; 20];
        for i in 0..20 {
            out[i] =
                u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(serde::de::Error::custom)?;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample_torrent_bytes() -> Vec<u8> {
        let pieces = b"abcdefghijklmnopqrst";
        let mut bytes = b"d8:announce35:udp://tracker.example:1337/announce4:infod5:filesld6:lengthi123e4:pathl7:foo.txteed6:lengthi456e4:pathl3:bar7:baz.mkveee4:name7:release12:piece lengthi16384e6:pieces20:".to_vec();
        bytes.extend_from_slice(pieces);
        bytes.extend_from_slice(b"ee");
        bytes
    }

    #[test]
    fn status_values_expose_stable_display_strings() {
        assert_eq!(DownloadStatus::Pending.as_str(), "pending");
        assert!(DownloadStatus::FetchingMetadata.is_active());
        assert!(!DownloadStatus::Seeding.is_terminal());
        assert!(DownloadStatus::Seeding.is_seeding());
        assert_eq!(ChunkStatus::Completed.as_str(), "completed");
    }

    #[test]
    fn progress_percent_guards() {
        let mut d = Download::new("u".into(), PathBuf::from("/p"));
        d.total_size = None;
        d.downloaded = 100;
        assert_eq!(d.progress_percent(), 0.0);
        d.total_size = Some(0);
        assert_eq!(d.progress_percent(), 0.0);
        d.total_size = Some(100);
        d.downloaded = 50;
        assert!((d.progress_percent() - 50.0).abs() < 0.01);
    }

    #[test]
    fn eta_seconds_behaviour() {
        let mut d = Download::new("u".into(), PathBuf::from("/p"));
        d.speed = 0;
        d.total_size = Some(1000);
        d.downloaded = 500;
        assert_eq!(d.eta_seconds(), None);

        d.speed = 100;
        d.total_size = None;
        assert_eq!(d.eta_seconds(), None);

        d.total_size = Some(1000);
        assert_eq!(d.eta_seconds(), Some(5));
    }

    #[test]
    fn file_path_with_and_without_subfolder() {
        let mut d = Download::new("u".into(), PathBuf::from("/root"));
        d.filename = "file.zip".into();
        assert_eq!(d.file_path(), PathBuf::from("/root/file.zip"));
        if let Some(http) = d.http_mut() {
            http.subfolder = Some("pkg".into());
        }
        assert_eq!(d.file_path(), PathBuf::from("/root/pkg/file.zip"));
        if let Some(http) = d.http_mut() {
            http.subfolder = Some(String::new());
        }
        assert_eq!(d.file_path(), PathBuf::from("/root/file.zip"));
    }

    #[test]
    fn torrent_file_path_uses_single_selected_file() {
        let source = TorrentSource::Magnet(
            "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862".into(),
        );
        let mut d =
            Download::try_from_torrent(source, PathBuf::from("/root")).expect("valid magnet");
        let torrent = d.torrent_mut().expect("torrent");
        torrent.files = vec![TorrentFile {
            path: PathBuf::from("folder/archive.zip"),
            size: 42,
            downloaded: 42,
            selected: true,
        }];
        assert_eq!(d.file_path(), PathBuf::from("/root/folder/archive.zip"));
        assert_eq!(d.file_dir(), PathBuf::from("/root/folder"));
    }

    #[test]
    fn torrent_file_dir_uses_common_root_when_multiple_files_selected() {
        let source = TorrentSource::Magnet(
            "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862".into(),
        );
        let mut d =
            Download::try_from_torrent(source, PathBuf::from("/root")).expect("valid magnet");
        let torrent = d.torrent_mut().expect("torrent");
        torrent.files = vec![
            TorrentFile {
                path: PathBuf::from("release/cd1.part01.rar"),
                size: 1,
                downloaded: 1,
                selected: true,
            },
            TorrentFile {
                path: PathBuf::from("release/cd1.part02.rar"),
                size: 1,
                downloaded: 1,
                selected: true,
            },
        ];
        assert_eq!(d.file_dir(), PathBuf::from("/root/release"));
    }

    #[test]
    fn try_from_torrent_constructs_torrent_kind() {
        let source = TorrentSource::Magnet(
            "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862".into(),
        );
        let d = Download::try_from_torrent(source, PathBuf::from("/tmp")).expect("valid magnet");
        assert!(d.kind.is_torrent());
        assert_eq!(d.filename, "Fetching metadata\u{2026}");
        assert!(!d.torrent().is_some_and(|torrent| torrent.auto_extract));
        assert_eq!(d.torrent().expect("torrent").metadata_bytes, None);
        assert_eq!(d.torrent().expect("torrent").metadata_wait_secs, 0);
    }

    #[test]
    fn try_from_torrent_rejects_invalid_source() {
        let source = TorrentSource::Magnet("not a magnet".into());
        assert!(Download::try_from_torrent(source, PathBuf::from("/tmp")).is_err());
    }

    #[test]
    fn try_from_torrent_file_uses_manifest_metadata() {
        let source = TorrentSource::File(sample_torrent_bytes());
        let d = Download::try_from_torrent(source, PathBuf::from("/tmp")).expect("torrent");

        assert_eq!(d.filename, "release");
        assert_eq!(d.total_size, Some(579));
        assert_eq!(d.torrent().expect("torrent").files.len(), 2);
    }

    #[test]
    fn debug_redacts_proxy_password_and_sensitive_headers() {
        let proxy = ProxyConfig {
            protocol: ProxyProtocol::Http,
            host: "proxy.example".to_string(),
            port: 8080,
            username: Some("user".to_string()),
            password: Some("secret".to_string()),
        };
        let http = HttpState {
            url: "https://example.com/file.bin?token=secret".to_string(),
            headers: vec![
                ("Authorization".to_string(), "Bearer secret".to_string()),
                ("Accept".to_string(), "application/octet-stream".to_string()),
            ],
            segments: 1,
            subfolder: None,
            auto_extract: false,
        };

        let debug = format!("{proxy:?} {http:?}");

        assert!(!debug.contains("secret"));
        assert!(debug.contains("<redacted>"));
        assert!(debug.contains("application/octet-stream"));
    }

    #[test]
    fn progress_detail_http_chunks_accessor() {
        let d = ProgressDetail::Http {
            chunks: vec![ChunkInfo {
                index: 0,
                start: 0,
                end: 9,
                downloaded: 5,
                status: ChunkStatus::Downloading,
            }],
        };
        let c = d.http_chunks().expect("http chunks");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].downloaded, 5);

        let e = ProgressDetail::empty_http();
        assert_eq!(e.http_chunks().map(<[_]>::len), Some(0));
    }
}
