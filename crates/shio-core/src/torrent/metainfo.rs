use std::fmt;
use std::num::NonZeroU64;
use std::path::{Component, PathBuf};

use crate::error::{Result, ShioError};
use crate::types::{
    MagnetPreviewManifest, MagnetPreviewResult, TorrentFile, TorrentSource, TorrentState,
};

pub const MAX_TORRENT_METADATA_BYTES: usize = 16 * 1024 * 1024;
const MAX_MAGNET_BYTES: usize = 16 * 1024;
const MAX_TORRENT_FILES: usize = 50_000;
const MAX_TORRENT_TRACKERS: usize = 256;
const MAX_TRACKER_LENGTH: usize = 2048;
const MAX_BENCODE_DEPTH: usize = 256;

#[derive(Clone, PartialEq, Eq)]
pub struct TorrentManifest {
    pub info_hash: [u8; 20],
    pub name: String,
    pub total_size: u64,
    pub is_private: bool,
    pub files: Vec<TorrentFile>,
    pub trackers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StagedFile {
    pub(super) index: usize,
    pub(super) path: PathBuf,
}

pub fn parse_torrent_manifest(bytes: &[u8]) -> Result<TorrentManifest> {
    parse_torrent(bytes).map(|(manifest, _)| manifest)
}

fn parse_torrent(bytes: &[u8]) -> Result<(TorrentManifest, Vec<StagedFile>)> {
    if bytes.len() > MAX_TORRENT_METADATA_BYTES || !bencode_depth_within_limit(bytes) {
        return Err(invalid(
            "torrent metadata is too large or too deeply nested",
        ));
    }
    let meta = librqbit::torrent_from_bytes(bytes)
        .map_err(|e| ShioError::Other(format!("invalid .torrent: {e}")))?;
    let raw = &meta.info.data;
    let total = match &raw.files {
        Some(files) => files
            .iter()
            .try_fold(0_u64, |total, file| total.checked_add(file.length)),
        None => raw.length,
    };
    // librqbit's `validate` divides by the piece length and keeps the piece count in a u32.
    let pieces = total
        .zip(NonZeroU64::new(raw.piece_length.into()))
        .map(|(total, piece)| total.div_ceil(piece.get()));
    if !pieces.is_some_and(|pieces| (1..=u64::from(u32::MAX)).contains(&pieces)) {
        return Err(invalid("torrent piece layout is invalid"));
    }
    let trackers = meta
        .iter_announce()
        .filter_map(|tracker| std::str::from_utf8(tracker.as_ref()).ok())
        .filter(|tracker| !tracker.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let info_hash = meta.info_hash.0;
    let info = meta
        .info
        .data
        .validate()
        .map_err(|e| ShioError::Other(format!("invalid torrent info: {e}")))?;

    let mut files = Vec::new();
    let mut staged = Vec::new();
    for (index, file) in info.iter_file_details().enumerate() {
        let attrs = file.attrs();
        if attrs.padding || attrs.symlink {
            continue;
        }
        if files.len() == MAX_TORRENT_FILES {
            return Err(invalid("torrent contains too many files"));
        }
        let components = file.filename.iter_components().collect::<Vec<_>>();
        let raw = file.filename.to_pathbuf();
        if raw.components().count() != components.len()
            || !raw.components().all(|c| matches!(c, Component::Normal(_)))
        {
            return Err(invalid("torrent file path escapes its folder"));
        }
        let path = components
            .iter()
            .map(|component| destination_component(component))
            .collect::<PathBuf>();
        crate::path::validate_relative_path(&path)?;
        files.push(TorrentFile {
            path,
            size: file.len,
            selected: true,
        });
        staged.push(StagedFile { index, path: raw });
    }
    validate_collections(&files, &trackers)?;

    let manifest = TorrentManifest {
        info_hash,
        name: crate::sanitize_filename(&info.name_or_else(|| "torrent")),
        total_size: files.iter().map(|file| file.size).sum(),
        is_private: info.info().private,
        files,
        trackers,
    };
    Ok((manifest, staged))
}

/// librqbit's bencode parser recurses per nested list/dict without a limit, so deep input would
/// overflow the stack. Malformed input passes here and fails in librqbit.
fn bencode_depth_within_limit(bytes: &[u8]) -> bool {
    let mut depth = 0_usize;
    let mut at = 0;
    while let Some(&byte) = bytes.get(at) {
        at += match byte {
            b'd' | b'l' => {
                depth += 1;
                if depth > MAX_BENCODE_DEPTH {
                    return false;
                }
                1
            },
            b'e' => {
                depth = depth.saturating_sub(1);
                1
            },
            b'i' => bytes[at..]
                .iter()
                .position(|&byte| byte == b'e')
                .map_or(bytes.len(), |end| end + 1),
            _ => {
                let Some(colon) = bytes[at..].iter().position(|&byte| byte == b':') else {
                    return true;
                };
                let Some(len) = std::str::from_utf8(&bytes[at..at + colon])
                    .ok()
                    .and_then(|len| len.parse::<usize>().ok())
                else {
                    return true;
                };
                (colon + 1).saturating_add(len)
            },
        };
    }
    true
}

fn destination_component(raw: &str) -> String {
    let mut name = raw
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, ':' | '<' | '>' | '"' | '|' | '?' | '*' | '\\') {
                '_'
            } else {
                c
            }
        })
        .collect::<String>();
    name.truncate(name.trim_end_matches(['.', ' ']).len());
    if name.is_empty() {
        name.push('_');
    }
    name
}

fn validate_collections(files: &[TorrentFile], trackers: &[String]) -> Result<()> {
    if files.is_empty() {
        return Err(invalid("torrent contains no files"));
    }
    if trackers.len() > MAX_TORRENT_TRACKERS {
        return Err(invalid("torrent contains too many trackers"));
    }
    if trackers
        .iter()
        .any(|tracker| tracker.len() > MAX_TRACKER_LENGTH)
    {
        return Err(invalid("torrent tracker URL is too long"));
    }
    let mut paths = files
        .iter()
        .map(|file| {
            file.path
                .components()
                .map(|component| component.as_os_str().to_string_lossy().to_lowercase())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    paths.sort_unstable();
    for pair in paths.windows(2) {
        if pair[0] == pair[1] {
            return Err(invalid("torrent contains colliding file paths"));
        }
        if pair[1].starts_with(&pair[0]) {
            return Err(invalid("torrent contains a file/directory path conflict"));
        }
    }
    Ok(())
}

pub(crate) fn parse_magnet_info_hash(magnet: &str) -> Result<[u8; 20]> {
    if magnet.len() > MAX_MAGNET_BYTES {
        return Err(invalid("magnet URI is too long"));
    }
    librqbit::Magnet::parse(magnet)
        .map_err(|e| ShioError::Other(format!("invalid magnet: {e}")))?
        .as_id20()
        .map(|hash| hash.0)
        .ok_or_else(|| invalid("magnet missing v1 info hash"))
}

pub(super) fn metadata_bytes(state: &TorrentState) -> Option<&[u8]> {
    match &state.source {
        TorrentSource::File(bytes) => Some(bytes),
        TorrentSource::Magnet(_) => state.metadata_bytes.as_deref(),
    }
}

/// The hash must match and the file list must be the manifest with only `selected` changed.
pub(crate) fn validate_torrent_state(state: &TorrentState) -> Result<()> {
    match metadata_bytes(state) {
        Some(bytes) => staged_files(state, bytes).map(drop),
        None if state.files.is_empty() => Ok(()),
        None => Err(invalid("torrent files are listed without metadata")),
    }
}

pub(super) fn staged_files(state: &TorrentState, bytes: &[u8]) -> Result<Vec<StagedFile>> {
    let (manifest, staged) = parse_torrent(bytes)?;
    if manifest.info_hash != state.info_hash {
        return Err(invalid("torrent metadata does not match its info hash"));
    }
    let same_files = manifest.files.len() == state.files.len()
        && manifest
            .files
            .iter()
            .zip(&state.files)
            .all(|(expected, file)| expected.path == file.path && expected.size == file.size);
    if !same_files {
        return Err(invalid("torrent file list does not match its metadata"));
    }
    if !state.files.iter().any(|file| file.selected) {
        return Err(invalid("torrent must have at least one selected file"));
    }
    Ok(staged)
}

fn invalid(message: &str) -> ShioError {
    ShioError::Other(message.into())
}

fn redacted_magnet(magnet: &str) -> String {
    parse_magnet_info_hash(magnet).map_or_else(
        |_| "Magnet(<invalid>)".into(),
        |hash| format!("Magnet({})", hex(&hash)),
    )
}

pub(super) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

impl fmt::Debug for TorrentManifest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TorrentManifest")
            .field("info_hash", &hex(&self.info_hash))
            .field("name", &self.name)
            .field("total_size", &self.total_size)
            .field("is_private", &self.is_private)
            .field("files", &self.files.len())
            .field("trackers", &self.trackers.len())
            .finish()
    }
}

impl fmt::Debug for TorrentSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Magnet(magnet) => f.write_str(&redacted_magnet(magnet)),
            Self::File(bytes) => write!(f, "File({} bytes)", bytes.len()),
        }
    }
}

impl fmt::Debug for TorrentState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TorrentState")
            .field("source", &self.source)
            .field("info_hash", &hex(&self.info_hash))
            .field(
                "metadata_bytes",
                &self.metadata_bytes.as_ref().map(Vec::len),
            )
            .field("auto_extract", &self.auto_extract)
            .field("is_private", &self.is_private)
            .field("files", &self.files.len())
            .field("trackers", &self.trackers.len())
            .field("stats", &self.stats)
            .finish()
    }
}

impl fmt::Debug for MagnetPreviewManifest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MagnetPreviewManifest")
            .field("name", &self.name)
            .field("total_size", &self.total_size)
            .field("is_private", &self.is_private)
            .field("files", &self.files.len())
            .field("trackers", &self.trackers.len())
            .field("metadata_bytes", &self.metadata_bytes.len())
            .finish()
    }
}

impl fmt::Debug for MagnetPreviewResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MagnetPreviewResult")
            .field("request_id", &self.request_id)
            .field("magnet", &redacted_magnet(&self.magnet))
            .field("result", &self.result)
            .finish()
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(in crate::torrent) fn torrent_download(
        bytes: Vec<u8>,
        save_path: PathBuf,
    ) -> crate::types::Download {
        let manifest = parse_torrent_manifest(&bytes).expect("torrent fixture");
        let mut state = TorrentState::new(TorrentSource::File(bytes), manifest.info_hash);
        state.files = manifest.files;
        crate::types::Download::with_kind(
            manifest.name,
            save_path,
            crate::types::DownloadKind::Torrent(state),
        )
    }

    pub(in crate::torrent) fn bencode_torrent(files: &[(&str, &[&str], u64)]) -> Vec<u8> {
        let mut list = Vec::new();
        for (attr, path, length) in files {
            list.extend_from_slice(b"d");
            if !attr.is_empty() {
                list.extend_from_slice(format!("4:attr{}:{attr}", attr.len()).as_bytes());
            }
            list.extend_from_slice(format!("6:lengthi{length}e4:pathl").as_bytes());
            for component in *path {
                list.extend_from_slice(format!("{}:{component}", component.len()).as_bytes());
            }
            list.extend_from_slice(b"ee");
        }
        let total: u64 = files.iter().map(|(_, _, length)| length).sum();
        let pieces = total.div_ceil(16_384).max(1);
        let mut bytes =
            b"d8:announce45:udp://tracker.example:1337/announce?pk=secret4:infod5:filesl".to_vec();
        bytes.extend_from_slice(&list);
        bytes.extend_from_slice(b"e4:name7:release12:piece lengthi16384e6:pieces");
        bytes.extend_from_slice(format!("{}:", pieces * 20).as_bytes());
        bytes.extend(std::iter::repeat_n(
            b'a',
            usize::try_from(pieces * 20).expect("small"),
        ));
        bytes.extend_from_slice(b"ee");
        bytes
    }

    #[test]
    fn padding_and_symlink_entries_are_skipped_with_index_mapping() {
        let bytes = bencode_torrent(&[
            ("", &["a.bin"], 100),
            ("p", &[".pad", "16284"], 16_284),
            ("l", &["link"], 0),
            ("", &["b.bin"], 50),
        ]);

        let (manifest, staged) = parse_torrent(&bytes).expect("manifest");

        let paths = manifest
            .files
            .iter()
            .map(|f| f.path.clone())
            .collect::<Vec<_>>();
        assert_eq!(paths, [PathBuf::from("a.bin"), PathBuf::from("b.bin")]);
        assert_eq!(manifest.total_size, 150);
        assert_eq!(
            staged.iter().map(|file| file.index).collect::<Vec<_>>(),
            [0, 3]
        );
    }

    #[test]
    fn portable_destination_names_keep_the_raw_staged_name() {
        let bytes = bencode_torrent(&[("", &["dir:x", "what?.mkv "], 10), ("", &["ok.txt"], 1)]);

        let (manifest, staged) = parse_torrent(&bytes).expect("manifest");

        assert_eq!(manifest.files[0].path, PathBuf::from("dir_x/what_.mkv"));
        assert_eq!(staged[0].path, PathBuf::from("dir:x/what?.mkv "));
        assert_eq!(manifest.files[1].path, PathBuf::from("ok.txt"));
    }

    #[test]
    fn hostile_metadata_is_rejected() {
        for files in [
            &[("", &["A.bin"][..], 1), ("", &["a.bin"][..], 1)][..],
            &[
                ("", &["release"][..], 1),
                ("", &["release", "data.bin"][..], 1),
            ],
            &[("", &["CON"][..], 1)],
            &[("p", &[".pad", "1"][..], 1)],
        ] {
            assert!(parse_torrent(&bencode_torrent(files)).is_err(), "{files:?}");
        }
        let valid = String::from_utf8(bencode_torrent(&[("", &["a"], 1), ("", &["b"], 2)]))
            .expect("ascii fixture");
        for (from, to) in [
            ("piece lengthi16384e", "piece lengthi0e"),
            ("6:lengthi1e", "6:lengthi18446744073709551615e"),
            ("6:lengthi1e", "6:lengthi70368744177664e"),
        ] {
            assert!(
                parse_torrent(valid.replacen(from, to, 1).as_bytes()).is_err(),
                "{to}"
            );
        }
        let mut deep = b"d4:infod1:x".to_vec();
        deep.extend(std::iter::repeat_n(b'l', 1_000_000));
        assert!(parse_torrent(&deep).is_err());
        assert!(parse_torrent(&[0_u8; 16]).is_err());
        assert!(parse_magnet_info_hash("not a magnet").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn drive_relative_component_is_rejected() {
        let bytes = bencode_torrent(&[("", &["C:evil.bin"], 1)]);
        assert!(parse_torrent(&bytes).is_err());
    }

    #[test]
    fn stored_state_must_match_its_metadata() {
        let bytes = bencode_torrent(&[("", &["a.bin"], 1), ("", &["b.bin"], 2)]);
        let manifest = parse_torrent_manifest(&bytes).expect("manifest");
        let mut state = TorrentState::new(TorrentSource::File(bytes), manifest.info_hash);
        state.files.clone_from(&manifest.files);
        assert!(validate_torrent_state(&state).is_ok());

        state.files[1].path = PathBuf::from("other.bin");
        assert!(validate_torrent_state(&state).is_err());

        state.files.clone_from(&manifest.files);
        state.info_hash = [0; 20];
        assert!(validate_torrent_state(&state).is_err());

        state.info_hash = manifest.info_hash;
        for file in &mut state.files {
            file.selected = false;
        }
        assert!(validate_torrent_state(&state).is_err());
    }
}
