use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::chunk::MAX_SEGMENTS;
use crate::config::AppConfig;
use crate::error::{Result, ShioError};
use crate::path::validate_leaf_filename;
use crate::types::{
    ArchivePackage, Download, DownloadId, DownloadKind, HttpState, PackageExtractState, PackageId,
    PackageItem, TorrentFile, TorrentSource, TorrentState,
};

const MAX_SOURCE_URL_BYTES: usize = 16 * 1024;
const MAX_ARCHIVE_PARTS: usize = 1_000;
const MAGNET_PLACEHOLDER_NAME: &str = "Fetching metadata\u{2026}";

#[derive(Clone)]
pub struct HttpDownloadRequest {
    id: DownloadId,
    url: String,
    save_path: PathBuf,
    filename: Option<String>,
    segments: Option<u8>,
    subfolder: Option<String>,
    auto_extract: bool,
}

impl HttpDownloadRequest {
    pub fn new(url: String, save_path: PathBuf) -> Self {
        Self {
            id: DownloadId::new(),
            url,
            save_path,
            filename: None,
            segments: None,
            subfolder: None,
            auto_extract: false,
        }
    }

    pub const fn id(&self) -> DownloadId {
        self.id
    }

    pub fn with_filename(mut self, filename: String) -> Self {
        self.filename = Some(filename);
        self
    }

    pub const fn with_segments(mut self, segments: u8) -> Self {
        self.segments = Some(segments);
        self
    }

    pub fn with_subfolder(mut self, subfolder: String) -> Self {
        self.subfolder = Some(subfolder);
        self
    }

    pub const fn with_auto_extract(mut self, auto_extract: bool) -> Self {
        self.auto_extract = auto_extract;
        self
    }

    pub(super) fn into_download(self, config: &AppConfig) -> Result<Download> {
        validate_http_url(&self.url)?;
        validate_save_path(&self.save_path)?;
        if let Some(subfolder) = self.subfolder.as_deref() {
            validate_leaf_filename(subfolder)?;
        }
        let segments = self.segments.unwrap_or(config.default_segments);
        if !(1..=MAX_SEGMENTS).contains(&segments) {
            return Err(ShioError::Other(format!(
                "segments must be between 1 and {MAX_SEGMENTS}"
            )));
        }
        let filename = self
            .filename
            .unwrap_or_else(|| crate::filename::extract_filename(&self.url));
        validate_leaf_filename(&filename)?;
        let mut http = HttpState::new(self.url);
        http.segments = segments;
        http.subfolder = self.subfolder;
        http.auto_extract = self.auto_extract;
        let mut download = Download::with_kind(filename, self.save_path, DownloadKind::Http(http));
        download.id = self.id;
        download.max_retries = config.max_retries;
        Ok(download)
    }
}

impl fmt::Debug for HttpDownloadRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpDownloadRequest")
            .field("id", &self.id)
            .field("url", &"<redacted>")
            .field("save_path", &self.save_path)
            .field("filename", &self.filename)
            .field("segments", &self.segments)
            .field("subfolder", &self.subfolder)
            .field("auto_extract", &self.auto_extract)
            .finish()
    }
}

#[derive(Clone)]
pub struct HttpArchivePartRequest {
    url: String,
    filename: String,
    part_number: u32,
    segments: u8,
}

impl HttpArchivePartRequest {
    pub fn new(url: String, filename: String, part_number: u32, segments: u8) -> Result<Self> {
        validate_http_url(&url)?;
        validate_leaf_filename(&filename)?;
        if part_number == 0 {
            return Err(ShioError::Other(
                "archive part number must be positive".into(),
            ));
        }
        Ok(Self {
            url,
            filename,
            part_number,
            segments,
        })
    }
}

impl fmt::Debug for HttpArchivePartRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpArchivePartRequest")
            .field("url", &"<redacted>")
            .field("filename", &self.filename)
            .field("part_number", &self.part_number)
            .field("segments", &self.segments)
            .finish()
    }
}

/// Package and part ids are fixed here, so every clone describes the same rows.
#[derive(Debug, Clone)]
pub struct HttpArchiveSetRequest {
    id: PackageId,
    name: String,
    save_path: PathBuf,
    auto_extract: bool,
    parts: Vec<(DownloadId, HttpArchivePartRequest)>,
}

impl HttpArchiveSetRequest {
    pub fn new(
        name: String,
        save_path: PathBuf,
        mut parts: Vec<HttpArchivePartRequest>,
        auto_extract: bool,
    ) -> Result<Self> {
        validate_leaf_filename(&name)?;
        validate_save_path(&save_path)?;
        if !(2..=MAX_ARCHIVE_PARTS).contains(&parts.len()) {
            return Err(ShioError::Other(format!(
                "archive set needs between 2 and {MAX_ARCHIVE_PARTS} parts"
            )));
        }
        parts.sort_by_key(|part| part.part_number);
        for (expected, part) in (1..).zip(&parts) {
            if part.part_number != expected {
                return Err(ShioError::Other(format!(
                    "archive part {expected:02} is missing or duplicated"
                )));
            }
        }
        let mut names_by_url = HashMap::new();
        for part in &parts {
            let url = crate::worker::url_without_fragment(&part.url);
            if let Some(existing) = names_by_url.insert(url, part.filename.as_str())
                && existing != part.filename
            {
                return Err(ShioError::Other(
                    "multiple filenames point to the same URL".into(),
                ));
            }
        }
        Ok(Self {
            id: PackageId::new(),
            name,
            save_path,
            auto_extract,
            parts: parts
                .into_iter()
                .map(|part| (DownloadId::new(), part))
                .collect(),
        })
    }

    pub const fn id(&self) -> PackageId {
        self.id
    }

    pub(super) fn into_rows(self, config: &AppConfig) -> Result<(ArchivePackage, Vec<Download>)> {
        let mut items = Vec::with_capacity(self.parts.len());
        let mut downloads = Vec::with_capacity(self.parts.len());
        for (position, (id, part)) in (0..).zip(self.parts) {
            let mut request = HttpDownloadRequest::new(part.url, self.save_path.clone())
                .with_filename(part.filename)
                .with_segments(part.segments)
                .with_subfolder(self.name.clone());
            request.id = id;
            downloads.push(request.into_download(config)?);
            items.push(PackageItem {
                download_id: id,
                position,
                part_number: part.part_number,
            });
        }
        let package = ArchivePackage {
            id: self.id,
            name: self.name,
            save_path: self.save_path,
            auto_extract: self.auto_extract,
            extract_state: PackageExtractState::NotStarted,
            error_message: None,
            created_at: chrono::Utc::now(),
            completed_at: None,
            pinned: false,
            items,
        };
        Ok((package, downloads))
    }
}

#[derive(Clone)]
pub struct TorrentDownloadRequest {
    id: DownloadId,
    source: TorrentSource,
    save_path: PathBuf,
    filename: Option<String>,
    files: Option<Vec<TorrentFile>>,
    metadata_bytes: Option<Vec<u8>>,
    auto_extract: bool,
}

impl TorrentDownloadRequest {
    pub fn new(source: TorrentSource, save_path: PathBuf) -> Self {
        Self {
            id: DownloadId::new(),
            source,
            save_path,
            filename: None,
            files: None,
            metadata_bytes: None,
            auto_extract: false,
        }
    }

    pub const fn id(&self) -> DownloadId {
        self.id
    }

    pub fn with_filename(mut self, filename: String) -> Self {
        self.filename = Some(filename);
        self
    }

    /// The torrent's files in manifest order; only `selected` may differ from the metadata.
    pub fn with_files(mut self, files: Vec<TorrentFile>) -> Self {
        self.files = Some(files);
        self
    }

    pub fn with_metadata_bytes(mut self, metadata_bytes: Vec<u8>) -> Self {
        self.metadata_bytes = Some(metadata_bytes);
        self
    }

    pub const fn with_auto_extract(mut self, auto_extract: bool) -> Self {
        self.auto_extract = auto_extract;
        self
    }

    pub(super) fn into_download(self) -> Result<Download> {
        validate_save_path(&self.save_path)?;
        let (info_hash, manifest) = match &self.source {
            TorrentSource::Magnet(magnet) => (
                crate::torrent::parse_magnet_info_hash(magnet)?,
                self.metadata_bytes
                    .as_deref()
                    .map(crate::torrent::parse_torrent_manifest)
                    .transpose()?,
            ),
            TorrentSource::File(bytes) => {
                let manifest = crate::torrent::parse_torrent_manifest(bytes)?;
                (manifest.info_hash, Some(manifest))
            },
        };
        let mut torrent = TorrentState::new(self.source, info_hash);
        torrent.auto_extract = self.auto_extract;
        torrent.metadata_bytes = self.metadata_bytes;
        let mut manifest_name = None;
        if let Some(manifest) = manifest {
            manifest_name = Some(crate::filename::sanitize_filename(&manifest.name));
            torrent.is_private = manifest.is_private;
            torrent.trackers = manifest.trackers;
            torrent.files = self.files.unwrap_or(manifest.files);
        }
        crate::torrent::validate_torrent_state(&torrent)?;
        let filename = self
            .filename
            .or(manifest_name)
            .unwrap_or_else(|| MAGNET_PLACEHOLDER_NAME.to_owned());
        validate_leaf_filename(&filename)?;
        let total_size = (!torrent.files.is_empty()).then(|| selected_size(&torrent.files));
        let mut download =
            Download::with_kind(filename, self.save_path, DownloadKind::Torrent(torrent));
        download.id = self.id;
        download.total_size = total_size;
        Ok(download)
    }
}

impl fmt::Debug for TorrentDownloadRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TorrentDownloadRequest")
            .field("id", &self.id)
            .field("source", &"<redacted>")
            .field("save_path", &self.save_path)
            .field("filename", &self.filename)
            .field("files", &self.files.as_ref().map(Vec::len))
            .field(
                "metadata_bytes",
                &self.metadata_bytes.as_ref().map(Vec::len),
            )
            .field("auto_extract", &self.auto_extract)
            .finish()
    }
}

pub(super) fn selected_size(files: &[TorrentFile]) -> u64 {
    files
        .iter()
        .filter(|file| file.selected)
        .map(|file| file.size)
        .fold(0, u64::saturating_add)
}

fn validate_http_url(url: &str) -> Result<()> {
    if url.len() > MAX_SOURCE_URL_BYTES {
        return Err(ShioError::Other("source URL is too long".into()));
    }
    let parsed =
        url::Url::parse(url).map_err(|error| ShioError::Other(format!("invalid URL: {error}")))?;
    if !matches!(parsed.scheme(), "http" | "https") || !parsed.has_host() {
        return Err(ShioError::Other(
            "URL must use http or https and include a host".into(),
        ));
    }
    Ok(())
}

fn validate_save_path(path: &Path) -> Result<()> {
    crate::path::path_to_utf8("save_path", path)?;
    if !path.is_absolute() {
        return Err(ShioError::Other("save path must be absolute".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(url: &str, name: &str, number: u32) -> HttpArchivePartRequest {
        HttpArchivePartRequest::new(url.into(), name.into(), number, 4).unwrap()
    }

    #[test]
    fn http_urls_need_an_http_scheme_and_host() {
        for url in ["http://example.com/a", "https://example.com/a"] {
            assert!(validate_http_url(url).is_ok(), "{url}");
        }
        for url in [
            "",
            "file:///tmp/a",
            "ftp://example.com/a",
            "magnet:?xt=urn:btih:abc",
            "/a",
            "http://",
        ] {
            assert!(validate_http_url(url).is_err(), "{url}");
        }
    }

    #[test]
    fn archive_set_rejects_gaps_duplicates_and_aliased_urls() {
        let save = std::env::temp_dir();
        for parts in [
            vec![
                part("https://e.com/1", "g.part01.rar", 1),
                part("https://e.com/3", "g.part03.rar", 3),
            ],
            vec![
                part("https://e.com/1", "g.part01.rar", 1),
                part("https://e.com/2", "g.part01.rar", 1),
            ],
            vec![
                part("https://e.com/x#a", "g.part01.rar", 1),
                part("https://e.com/x#b", "g.part02.rar", 2),
            ],
        ] {
            assert!(HttpArchiveSetRequest::new("g".into(), save.clone(), parts, true).is_err());
        }
    }

    #[test]
    fn archive_set_rows_are_identical_for_every_clone() {
        let request = HttpArchiveSetRequest::new(
            "game".into(),
            std::env::temp_dir(),
            vec![
                part("https://e.com/2", "game.part02.rar", 2),
                part("https://e.com/1", "game.part01.rar", 1),
            ],
            true,
        )
        .unwrap();
        let config = AppConfig::default();

        let (first_package, first) = request.clone().into_rows(&config).unwrap();
        let (second_package, second) = request.into_rows(&config).unwrap();

        assert_eq!(first_package.id, second_package.id);
        assert_eq!(first_package.items, second_package.items);
        assert_eq!(
            first.iter().map(|d| d.id).collect::<Vec<_>>(),
            second.iter().map(|d| d.id).collect::<Vec<_>>()
        );
        assert_eq!(first[0].filename, "game.part01.rar");
    }
}
