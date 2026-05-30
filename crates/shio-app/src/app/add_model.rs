use super::state::{
    AddHttpPreview, AddHttpPreviewState, AddMagnetPreview, AddMagnetPreviewState, AddSourceId,
    AddTorrentFile, AddTorrentSource,
};
use shio_core::TorrentFile;
use std::path::{Path, PathBuf};

pub(crate) const ADD_URL_PREVIEW_LIMIT: usize = 20;
pub(crate) const HTTP_PREVIEW_BATCH_LIMIT: usize = 8;

pub(crate) fn is_magnet(url: &str) -> bool {
    url.trim_start().to_ascii_lowercase().starts_with("magnet:")
}

fn is_http_url(url: &str) -> bool {
    url::Url::parse(url)
        .is_ok_and(|parsed| matches!(parsed.scheme(), "http" | "https") && parsed.has_host())
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ParsedAddUrls {
    pub(crate) entries: Vec<String>,
    pub(crate) entry_previews: Vec<String>,
    pub(crate) http_urls: Vec<String>,
    pub(crate) magnets: Vec<String>,
    pub(crate) preview: Vec<String>,
    pub(crate) http_count: usize,
    pub(crate) magnet_count: usize,
    pub(crate) single_url_name: Option<String>,
    pub(crate) has_archive_url: bool,
    pub(crate) suggested_subfolder: Option<String>,
}

pub(crate) fn parse_add_urls(text: &str) -> ParsedAddUrls {
    let mut parsed = ParsedAddUrls::default();
    let mut filenames = Vec::new();

    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if is_magnet(line) {
            parsed.entries.push(line.to_string());
            parsed.magnet_count += 1;
            parsed.magnets.push(line.to_string());
            parsed.entry_previews.push("magnet link".to_string());
            if parsed.preview.len() < ADD_URL_PREVIEW_LIMIT {
                parsed.preview.push("magnet link".to_string());
            }
            continue;
        }

        if is_http_url(line) {
            parsed.entries.push(line.to_string());
            parsed.http_count += 1;
            parsed.http_urls.push(line.to_string());
            let filename = shio_core::extract_filename(line, None);
            parsed.has_archive_url |= shio_core::is_archive_filename(&filename);
            parsed.entry_previews.push(filename.clone());
            if parsed.preview.len() < ADD_URL_PREVIEW_LIMIT {
                parsed.preview.push(filename.clone());
            }
            filenames.push(filename);
        }
    }

    if parsed.entries.len() == 1 && parsed.http_count == 1 && parsed.magnet_count == 0 {
        parsed.single_url_name = filenames.first().cloned();
    }

    let filename_refs: Vec<&str> = filenames.iter().map(String::as_str).collect();
    parsed.suggested_subfolder = shio_core::suggest_folder_name(&filename_refs);

    parsed
}

pub(crate) fn path_label(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn selected_torrent_files(torrent: &AddTorrentFile) -> Vec<&TorrentFile> {
    torrent.files.iter().filter(|file| file.selected).collect()
}

fn torrent_file_matches(file: &TorrentFile, query: &str) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return true;
    }

    let query = query.to_ascii_lowercase();
    file.path
        .to_string_lossy()
        .to_ascii_lowercase()
        .contains(&query)
}

pub(crate) fn filtered_torrent_file_indices(torrent: &AddTorrentFile, query: &str) -> Vec<usize> {
    torrent
        .files
        .iter()
        .enumerate()
        .filter_map(|(index, file)| torrent_file_matches(file, query).then_some(index))
        .collect()
}

pub(crate) fn set_matching_torrent_files(
    torrent: &mut AddTorrentFile,
    query: &str,
    selected: bool,
) {
    for file_index in filtered_torrent_file_indices(torrent, query) {
        if let Some(file) = torrent.files.get_mut(file_index) {
            file.selected = selected;
        }
    }
}

pub(crate) fn default_add_source_selection(
    urls: &[String],
    torrent_files: &[AddTorrentFile],
) -> Option<AddSourceId> {
    torrent_files
        .iter()
        .find(|torrent| torrent.selected_count() == 0)
        .or_else(|| torrent_files.first())
        .map(|torrent| match &torrent.source {
            AddTorrentSource::File { path } => AddSourceId::TorrentFile(path.clone()),
            AddTorrentSource::Magnet { request_id, .. } => AddSourceId::MagnetPreview(*request_id),
        })
        .or_else(|| (!urls.is_empty()).then_some(AddSourceId::Url(0)))
}

pub(crate) fn add_source_exists(
    source: &AddSourceId,
    urls: &[String],
    torrent_files: &[AddTorrentFile],
) -> bool {
    match source {
        AddSourceId::Url(index) => *index < urls.len(),
        AddSourceId::TorrentFile(path) => torrent_files.iter().any(|torrent| {
            matches!(&torrent.source, AddTorrentSource::File { path: source_path } if source_path == path)
        }),
        AddSourceId::MagnetPreview(request_id) => torrent_files.iter().any(|torrent| {
            matches!(
                &torrent.source,
                AddTorrentSource::Magnet {
                    request_id: source_request_id,
                    ..
                } if source_request_id == request_id
            )
        }),
    }
}

pub(crate) fn repair_add_source_selection(
    selected_source: &mut Option<AddSourceId>,
    urls: &[String],
    torrent_files: &[AddTorrentFile],
) {
    if selected_source
        .as_ref()
        .is_some_and(|source| add_source_exists(source, urls, torrent_files))
    {
        return;
    }

    *selected_source = default_add_source_selection(urls, torrent_files);
}

pub(crate) fn can_confirm_add_sources(urls: &[String], torrent_files: &[AddTorrentFile]) -> bool {
    (!urls.is_empty() || !torrent_files.is_empty())
        && torrent_files.iter().all(AddTorrentFile::has_selection)
}

pub(crate) const fn should_request_http_previews(http_urls: &[String]) -> bool {
    !http_urls.is_empty() && http_urls.len() <= HTTP_PREVIEW_BATCH_LIMIT
}

fn http_preview_for_url<'a>(
    url: &str,
    http_previews: &'a [AddHttpPreview],
) -> Option<&'a AddHttpPreviewState> {
    http_previews
        .iter()
        .find(|preview| preview.url == url)
        .map(|preview| &preview.state)
}

pub(crate) fn http_preview_name(url: &str, http_previews: &[AddHttpPreview]) -> Option<String> {
    match http_preview_for_url(url, http_previews) {
        Some(AddHttpPreviewState::Ready(preview)) if !preview.filename.is_empty() => {
            Some(preview.filename.clone())
        },
        _ => None,
    }
}

pub(crate) fn add_single_http_name(
    parsed: &ParsedAddUrls,
    http_previews: &[AddHttpPreview],
) -> Option<String> {
    if parsed.entries.len() != 1 || parsed.http_count != 1 || parsed.magnet_count != 0 {
        return None;
    }
    let url = parsed.http_urls.first()?;
    http_preview_name(url, http_previews).or_else(|| parsed.single_url_name.clone())
}

pub(crate) fn addable_url_entries(
    urls: &[String],
    http_previews: &[AddHttpPreview],
) -> Vec<String> {
    urls.iter()
        .filter(|url| {
            is_magnet(url)
                || (is_http_url(url)
                    && !matches!(
                        http_preview_for_url(url, http_previews),
                        Some(AddHttpPreviewState::Blocked { .. })
                    ))
        })
        .cloned()
        .collect()
}

pub(crate) fn addable_has_archive_url(urls: &[String], http_previews: &[AddHttpPreview]) -> bool {
    addable_url_entries(urls, http_previews)
        .iter()
        .filter(|url| !is_magnet(url))
        .map(|url| {
            http_preview_name(url, http_previews)
                .unwrap_or_else(|| shio_core::extract_filename(url, None))
        })
        .any(|filename| shio_core::is_archive_filename(&filename))
}

pub(crate) fn url_without_fragment(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(mut parsed) => {
            parsed.set_fragment(None);
            parsed.to_string()
        },
        Err(_) => url.to_string(),
    }
}

pub(crate) fn archive_set_validation_error(
    urls: &[String],
    http_previews: &[AddHttpPreview],
) -> Option<String> {
    let mut groups: std::collections::BTreeMap<String, Vec<u32>> =
        std::collections::BTreeMap::new();
    let mut filenames_by_network_url = std::collections::BTreeMap::<String, String>::new();
    for url in urls.iter().filter(|url| !is_magnet(url)) {
        let filename = http_preview_name(url, http_previews)
            .unwrap_or_else(|| shio_core::extract_filename(url, None));
        let network_url = url_without_fragment(url);
        if let Some(existing) = filenames_by_network_url.insert(network_url, filename.clone())
            && existing != filename
        {
            return Some("multiple filenames point to the same URL".to_string());
        }
        if let Some(part) = shio_core::parse_archive_part(&filename) {
            groups
                .entry(part.base_name)
                .or_default()
                .push(part.part_number);
        }
    }
    for mut parts in groups.into_values().filter(|parts| parts.len() >= 2) {
        parts.sort_unstable();
        for (index, part) in parts.into_iter().enumerate() {
            let expected = u32::try_from(index + 1).ok()?;
            if part != expected {
                return Some(format!("missing part{expected:02}.rar"));
            }
        }
    }
    None
}

pub(crate) fn removed_http_preview_request_ids(
    http_previews: &[AddHttpPreview],
    parsed_http_urls: &[String],
) -> Vec<u64> {
    http_previews
        .iter()
        .filter(|preview| !parsed_http_urls.contains(&preview.url))
        .map(|preview| preview.request_id)
        .collect()
}

pub(crate) fn drain_http_preview_request_ids(http_previews: &mut Vec<AddHttpPreview>) -> Vec<u64> {
    http_previews
        .drain(..)
        .map(|preview| preview.request_id)
        .collect()
}

pub(crate) fn apply_http_preview_result(
    result: shio_core::HttpPreviewResult,
    parsed: &ParsedAddUrls,
    http_previews: &mut [AddHttpPreview],
) {
    let Some(position) = http_previews
        .iter()
        .position(|preview| preview.request_id == result.request_id)
    else {
        return;
    };

    if !parsed
        .http_urls
        .iter()
        .any(|url| url == &http_previews[position].url)
    {
        return;
    }

    http_previews[position].state = match result.state {
        shio_core::HttpPreviewState::Ready(preview) => AddHttpPreviewState::Ready(preview),
        shio_core::HttpPreviewState::Blocked { reason } => AddHttpPreviewState::Blocked { reason },
        shio_core::HttpPreviewState::Error { message } => AddHttpPreviewState::Failed { message },
    };
}

pub(crate) fn apply_magnet_preview_result(
    result: shio_core::MagnetPreviewResult,
    parsed: &ParsedAddUrls,
    url_entries: &mut Vec<String>,
    magnet_previews: &mut Vec<AddMagnetPreview>,
    torrent_files: &mut Vec<AddTorrentFile>,
) {
    if !parsed.magnets.iter().any(|magnet| magnet == &result.magnet) {
        return;
    }

    let Some(position) = magnet_previews.iter().position(|preview| {
        preview.magnet == result.magnet && preview.request_id == result.request_id
    }) else {
        return;
    };

    match result.result {
        Ok(manifest) => {
            magnet_previews.remove(position);
            url_entries.retain(|entry| entry != &result.magnet);
            torrent_files.push(AddTorrentFile {
                source: AddTorrentSource::Magnet {
                    magnet: result.magnet,
                    request_id: result.request_id,
                },
                display_name: manifest.name,
                is_private: manifest.is_private,
                files: manifest.files,
                trackers: manifest.trackers,
                metadata_bytes: Some(manifest.metadata_bytes),
            });
        },
        Err(message) => {
            magnet_previews[position].state = AddMagnetPreviewState::Failed { message };
        },
    }
}

pub(crate) fn torrent_display_name(torrent: &AddTorrentFile) -> String {
    let selected = selected_torrent_files(torrent);
    if selected.len() == 1 {
        return selected[0]
            .path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .map_or_else(
                || torrent.display_name.clone(),
                shio_core::sanitize_filename,
            );
    }
    torrent.display_name.clone()
}

pub(crate) fn torrent_subfolder_name(torrent: &AddTorrentFile) -> Option<String> {
    let selected = selected_torrent_files(torrent);
    if selected.is_empty() {
        return None;
    }
    if selected.len() == 1 {
        return selected[0]
            .path
            .file_stem()
            .and_then(std::ffi::OsStr::to_str)
            .map(shio_core::sanitize_filename)
            .filter(|name| !name.is_empty());
    }

    let mut components = selected
        .iter()
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
    if !shared.is_empty() {
        let mut out = PathBuf::new();
        for component in shared {
            out.push(component.as_os_str());
        }
        if let Some(name) = out.file_name().and_then(std::ffi::OsStr::to_str) {
            let sanitized = shio_core::sanitize_filename(name);
            if !sanitized.is_empty() {
                return Some(sanitized);
            }
        }
    }
    Some(torrent.display_name.clone()).filter(|name| !name.is_empty())
}

pub(crate) fn torrent_file_preview(
    path: PathBuf,
    bytes: Vec<u8>,
) -> Result<AddTorrentFile, String> {
    let manifest = shio_core::parse_torrent_manifest(&bytes)
        .map_err(|error| format!("invalid torrent file {}: {error}", path_label(&path)))?;
    Ok(AddTorrentFile {
        source: AddTorrentSource::File { path },
        display_name: manifest.name,
        is_private: manifest.is_private,
        files: manifest.files,
        trackers: manifest.trackers,
        metadata_bytes: Some(bytes),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{
        AddHttpPreview, AddHttpPreviewState, AddMagnetPreview, AddMagnetPreviewState,
        AddTorrentPreview, AddTorrentSource,
    };
    use shio_core::{
        HttpPreview, HttpPreviewResult, HttpPreviewState, MagnetPreviewManifest,
        MagnetPreviewResult,
    };

    const MAGNET: &str = "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862&dn=ubuntu";
    const HTTP_URL: &str = "https://example.com/download?id=1";
    const OTHER_HTTP_URL: &str = "https://example.com/file.bin";

    fn torrent_with_selection(selected: bool) -> AddTorrentPreview {
        AddTorrentPreview {
            source: AddTorrentSource::File {
                path: PathBuf::from("batch.torrent"),
            },
            display_name: "batch".to_string(),
            is_private: false,
            files: vec![TorrentFile {
                path: PathBuf::from("file.bin"),
                size: 1,
                downloaded: 0,
                selected,
            }],
            trackers: Vec::new(),
            metadata_bytes: None,
        }
    }

    fn parsed_with_magnet() -> ParsedAddUrls {
        parse_add_urls(&format!("https://example.com/file.bin\n{MAGNET}"))
    }

    fn parsed_with_http_urls() -> ParsedAddUrls {
        parse_add_urls(&format!("{HTTP_URL}\n{OTHER_HTTP_URL}"))
    }

    fn preview_result(request_id: u64) -> MagnetPreviewResult {
        MagnetPreviewResult {
            request_id,
            magnet: MAGNET.to_string(),
            result: Ok(MagnetPreviewManifest {
                name: "ubuntu".to_string(),
                total_size: 42,
                is_private: false,
                files: vec![TorrentFile {
                    path: PathBuf::from("ubuntu.iso"),
                    size: 42,
                    downloaded: 0,
                    selected: true,
                }],
                trackers: vec!["udp://tracker.example:1337/announce".to_string()],
                metadata_bytes: b"metadata".to_vec(),
            }),
        }
    }

    fn http_ready_result(request_id: u64) -> HttpPreviewResult {
        HttpPreviewResult {
            request_id,
            state: HttpPreviewState::Ready(HttpPreview {
                filename: "server-name.iso".to_string(),
                total_size: Some(42),
                content_type: Some("application/octet-stream".to_string()),
                accept_ranges: true,
            }),
        }
    }

    fn sample_torrent_bytes() -> Vec<u8> {
        let pieces = b"abcdefghijklmnopqrst";
        let mut bytes = b"d8:announce35:udp://tracker.example:1337/announce4:infod5:filesld6:lengthi123e4:pathl7:foo.txteed6:lengthi456e4:pathl3:bar7:baz.mkveee4:name7:release12:piece lengthi16384e6:pieces20:".to_vec();
        bytes.extend_from_slice(pieces);
        bytes.extend_from_slice(b"ee");
        bytes
    }

    #[test]
    fn parse_add_urls_uses_fragment_filenames() {
        let input = [
            "https://example.com/a#archive.part01.rar",
            "https://example.com/b#archive.part02.rar",
            "magnet:?xt=urn:btih:123",
        ]
        .join("\n");

        let parsed = parse_add_urls(&input);

        assert_eq!(parsed.entries.len(), 3);
        assert_eq!(parsed.http_count, 2);
        assert_eq!(parsed.magnet_count, 1);
        assert_eq!(parsed.preview[0], "archive.part01.rar");
        assert_eq!(parsed.preview[1], "archive.part02.rar");
        assert_eq!(parsed.preview[2], "magnet link");
        assert!(parsed.has_archive_url);
        assert_eq!(parsed.suggested_subfolder.as_deref(), Some("archive"));
    }

    #[test]
    fn parse_add_urls_caps_preview() {
        let input = (1..=32)
            .map(|n| format!("https://example.com/{n}#batch.part{n:02}.rar"))
            .collect::<Vec<_>>()
            .join("\n");

        let parsed = parse_add_urls(&input);

        assert_eq!(parsed.entries.len(), 32);
        assert_eq!(parsed.preview.len(), ADD_URL_PREVIEW_LIMIT);
        assert_eq!(parsed.http_count, 32);
        assert_eq!(parsed.magnet_count, 0);
    }

    #[test]
    fn parse_add_urls_ignores_invalid_intermediate_http_text() {
        let input = [
            HTTP_URL,
            "https://",
            "not a url",
            "ftp://example.com/file.bin",
            MAGNET,
        ]
        .join("\n");

        let parsed = parse_add_urls(&input);

        assert_eq!(
            parsed.entries,
            vec![HTTP_URL.to_string(), MAGNET.to_string()]
        );
        assert_eq!(parsed.http_urls, vec![HTTP_URL.to_string()]);
        assert_eq!(parsed.http_count, 1);
        assert_eq!(parsed.magnet_count, 1);
        assert_eq!(
            parsed.preview,
            vec!["download".to_string(), "magnet link".to_string()]
        );
    }

    #[test]
    fn can_confirm_add_sources_rejects_empty_sources() {
        assert!(!can_confirm_add_sources(&[], &[]));
    }

    #[test]
    fn can_confirm_add_sources_rejects_torrent_without_selection() {
        let torrents = [torrent_with_selection(false)];

        assert!(!can_confirm_add_sources(
            &["https://example.com/file.bin".to_string()],
            &torrents
        ));
    }

    #[test]
    fn can_confirm_add_sources_accepts_urls_and_selected_torrents() {
        let torrents = [torrent_with_selection(true)];

        assert!(can_confirm_add_sources(
            &["https://example.com/file.bin".to_string()],
            &torrents
        ));
    }

    #[test]
    fn default_source_selection_uses_url_row_index() {
        assert_eq!(
            default_add_source_selection(&[HTTP_URL.to_string(), HTTP_URL.to_string()], &[]),
            Some(AddSourceId::Url(0))
        );
    }

    #[test]
    fn url_source_identity_allows_duplicate_urls_by_index() {
        let urls = vec![HTTP_URL.to_string(), HTTP_URL.to_string()];

        assert!(add_source_exists(&AddSourceId::Url(0), &urls, &[]));
        assert!(add_source_exists(&AddSourceId::Url(1), &urls, &[]));
        assert!(!add_source_exists(&AddSourceId::Url(2), &urls, &[]));
    }

    #[test]
    fn torrent_file_filter_matches_path_and_preserves_original_indices() {
        let torrent = AddTorrentPreview {
            source: AddTorrentSource::File {
                path: PathBuf::from("release.torrent"),
            },
            display_name: "release".to_string(),
            is_private: false,
            files: vec![
                TorrentFile {
                    path: PathBuf::from("show/s01e01.mkv"),
                    size: 10,
                    downloaded: 0,
                    selected: true,
                },
                TorrentFile {
                    path: PathBuf::from("subs/episode-one.srt"),
                    size: 1,
                    downloaded: 0,
                    selected: true,
                },
                TorrentFile {
                    path: PathBuf::from("extras/Poster.JPG"),
                    size: 2,
                    downloaded: 0,
                    selected: true,
                },
            ],
            trackers: Vec::new(),
            metadata_bytes: None,
        };

        assert_eq!(filtered_torrent_file_indices(&torrent, "subs"), vec![1]);
        assert_eq!(
            filtered_torrent_file_indices(&torrent, "poster.jpg"),
            vec![2]
        );
    }

    #[test]
    fn torrent_matching_selection_changes_all_matching_original_indices() {
        let mut torrent = AddTorrentPreview {
            source: AddTorrentSource::File {
                path: PathBuf::from("release.torrent"),
            },
            display_name: "release".to_string(),
            is_private: false,
            files: vec![
                TorrentFile {
                    path: PathBuf::from("keep/a.txt"),
                    size: 1,
                    downloaded: 0,
                    selected: true,
                },
                TorrentFile {
                    path: PathBuf::from("drop/b.txt"),
                    size: 1,
                    downloaded: 0,
                    selected: true,
                },
                TorrentFile {
                    path: PathBuf::from("keep/c.txt"),
                    size: 1,
                    downloaded: 0,
                    selected: true,
                },
            ],
            trackers: Vec::new(),
            metadata_bytes: None,
        };

        set_matching_torrent_files(&mut torrent, "keep", false);

        assert_eq!(
            torrent
                .files
                .iter()
                .map(|file| file.selected)
                .collect::<Vec<_>>(),
            vec![false, true, false]
        );
    }

    #[test]
    fn resolved_magnet_moves_into_torrent_previews() {
        let mut url_entries = parsed_with_magnet().entries;
        let mut magnets = vec![AddMagnetPreview {
            magnet: MAGNET.to_string(),
            request_id: 7,
            state: AddMagnetPreviewState::Resolving,
        }];
        let mut torrents = Vec::new();

        apply_magnet_preview_result(
            preview_result(7),
            &parsed_with_magnet(),
            &mut url_entries,
            &mut magnets,
            &mut torrents,
        );

        assert_eq!(url_entries, vec!["https://example.com/file.bin"]);
        assert!(magnets.is_empty());
        assert_eq!(torrents.len(), 1);
        assert_eq!(torrents[0].display_name, "ubuntu");
        assert_eq!(torrents[0].selected_count(), 1);
        assert_eq!(
            torrents[0].metadata_bytes.as_deref(),
            Some(&b"metadata"[..])
        );
    }

    #[test]
    fn stale_magnet_preview_result_is_ignored() {
        let mut url_entries = parsed_with_magnet().entries;
        let mut magnets = vec![AddMagnetPreview {
            magnet: MAGNET.to_string(),
            request_id: 8,
            state: AddMagnetPreviewState::Resolving,
        }];
        let mut torrents = Vec::new();

        apply_magnet_preview_result(
            preview_result(7),
            &parsed_with_magnet(),
            &mut url_entries,
            &mut magnets,
            &mut torrents,
        );

        assert_eq!(url_entries.len(), 2);
        assert_eq!(magnets.len(), 1);
        assert!(torrents.is_empty());
    }

    #[test]
    fn failed_magnet_preview_remains_addable() {
        let mut url_entries = parsed_with_magnet().entries;
        let mut magnets = vec![AddMagnetPreview {
            magnet: MAGNET.to_string(),
            request_id: 7,
            state: AddMagnetPreviewState::Resolving,
        }];
        let mut torrents = Vec::new();

        apply_magnet_preview_result(
            MagnetPreviewResult {
                request_id: 7,
                magnet: MAGNET.to_string(),
                result: Err("metadata unavailable".to_string()),
            },
            &parsed_with_magnet(),
            &mut url_entries,
            &mut magnets,
            &mut torrents,
        );

        assert!(url_entries.iter().any(|entry| entry == MAGNET));
        assert!(matches!(
            magnets[0].state,
            AddMagnetPreviewState::Failed { .. }
        ));
        assert!(torrents.is_empty());
    }

    #[test]
    fn stale_http_preview_result_is_ignored() {
        let mut previews = vec![AddHttpPreview {
            url: HTTP_URL.to_string(),
            request_id: 8,
            state: AddHttpPreviewState::Resolving,
        }];

        apply_http_preview_result(
            http_ready_result(7),
            &parsed_with_http_urls(),
            &mut previews,
        );

        assert_eq!(
            previews,
            vec![AddHttpPreview {
                url: HTTP_URL.to_string(),
                request_id: 8,
                state: AddHttpPreviewState::Resolving,
            }]
        );
    }

    #[test]
    fn ready_http_preview_updates_name_without_removing_original_url() {
        let parsed = parsed_with_http_urls();
        let url_entries = parsed.entries.clone();
        let mut previews = vec![AddHttpPreview {
            url: HTTP_URL.to_string(),
            request_id: 7,
            state: AddHttpPreviewState::Resolving,
        }];

        apply_http_preview_result(http_ready_result(7), &parsed, &mut previews);

        assert_eq!(url_entries[0], HTTP_URL);
        assert!(matches!(
            previews[0].state,
            AddHttpPreviewState::Ready(HttpPreview { ref filename, .. }) if filename == "server-name.iso"
        ));
    }

    #[test]
    fn blocked_http_preview_is_not_addable() {
        let parsed = parsed_with_http_urls();
        let mut previews = vec![
            AddHttpPreview {
                url: HTTP_URL.to_string(),
                request_id: 7,
                state: AddHttpPreviewState::Resolving,
            },
            AddHttpPreview {
                url: OTHER_HTTP_URL.to_string(),
                request_id: 8,
                state: AddHttpPreviewState::Resolving,
            },
        ];

        apply_http_preview_result(http_ready_result(7), &parsed, &mut previews);
        apply_http_preview_result(
            HttpPreviewResult {
                request_id: 8,
                state: HttpPreviewState::Blocked {
                    reason: "unsupported host".to_string(),
                },
            },
            &parsed,
            &mut previews,
        );

        let addable_urls = addable_url_entries(&parsed.entries, &previews);

        assert_eq!(addable_urls, vec![HTTP_URL.to_string()]);
        assert!(can_confirm_add_sources(&addable_urls, &[]));
        assert_eq!(previews.len(), 2);
        assert!(matches!(
            previews[1].state,
            AddHttpPreviewState::Blocked { ref reason } if reason == "unsupported host"
        ));
    }

    #[test]
    fn torrent_file_preview_retains_source_bytes() {
        let bytes = sample_torrent_bytes();
        let path = PathBuf::from("sample.torrent");

        let preview = torrent_file_preview(path, bytes.clone()).expect("valid torrent");

        assert_eq!(preview.metadata_bytes.as_deref(), Some(bytes.as_slice()));
    }

    #[test]
    fn failed_http_preview_does_not_block_other_valid_urls() {
        let parsed = parsed_with_http_urls();
        let mut previews = vec![
            AddHttpPreview {
                url: HTTP_URL.to_string(),
                request_id: 7,
                state: AddHttpPreviewState::Resolving,
            },
            AddHttpPreview {
                url: OTHER_HTTP_URL.to_string(),
                request_id: 8,
                state: AddHttpPreviewState::Resolving,
            },
        ];

        apply_http_preview_result(http_ready_result(7), &parsed, &mut previews);
        apply_http_preview_result(
            HttpPreviewResult {
                request_id: 8,
                state: HttpPreviewState::Error {
                    message: "preview timed out".to_string(),
                },
            },
            &parsed,
            &mut previews,
        );

        let addable_urls = addable_url_entries(&parsed.entries, &previews);

        assert_eq!(
            addable_urls,
            vec![HTTP_URL.to_string(), OTHER_HTTP_URL.to_string()]
        );
        assert!(can_confirm_add_sources(&addable_urls, &[]));
        assert_eq!(previews.len(), 2);
        assert!(matches!(
            previews[1].state,
            AddHttpPreviewState::Failed { ref message } if message == "preview timed out"
        ));
    }

    #[test]
    fn draining_http_preview_request_ids_clears_pending_previews() {
        let mut previews = vec![
            AddHttpPreview {
                url: HTTP_URL.to_string(),
                request_id: 7,
                state: AddHttpPreviewState::Resolving,
            },
            AddHttpPreview {
                url: OTHER_HTTP_URL.to_string(),
                request_id: 8,
                state: AddHttpPreviewState::Ready(HttpPreview {
                    filename: "server-name.iso".to_string(),
                    total_size: Some(42),
                    content_type: Some("application/octet-stream".to_string()),
                    accept_ranges: true,
                }),
            },
        ];

        let request_ids = drain_http_preview_request_ids(&mut previews);

        assert_eq!(request_ids, vec![7, 8]);
        assert!(previews.is_empty());
    }

    #[test]
    fn large_http_batches_skip_network_previews() {
        let urls = (1..=46)
            .map(|n| format!("https://example.com/file.part{n:02}.rar"))
            .collect::<Vec<_>>();

        assert!(!should_request_http_previews(&urls));
    }
}
